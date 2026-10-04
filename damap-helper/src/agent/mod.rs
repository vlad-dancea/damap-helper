//! The AI reviewer: asks a language model whether a batch of changes to the
//! project folder contradicts the data management plan.
//!
//! This is a deliberately small agent loop. The model gets the DMP and the
//! list of changes, may look around the folder with read-only tools, and must
//! finish by calling `report_verdict`. Acting on the verdict (undo, or change
//! the DMP) is left to the caller, so the model itself can never alter anything.
//!
//! The model is reached through an OpenAI-compatible API, which Ollama, LM
//! Studio, vLLM, OpenRouter, OpenAI and most university LLM gateways offer.
//! The `genai` crate speaks the protocol, tool calls included.

mod tools;

use std::path::Path;

use anyhow::{Context, Result, bail};
use genai::adapter::AdapterKind;
use genai::chat::{ChatMessage, ChatOptions, ChatRequest, Tool, ToolChoice, ToolResponse};
use genai::resolver::{AuthData, Endpoint};
use genai::{Client, ModelIden, ServiceTarget};
use reqwest::blocking::Client as HttpClient;
use serde::Deserialize;
use serde_json::json;
use tokio::runtime::Runtime;

use crate::config::AiConfig;
use crate::watcher::Change;
use tools::Tools;

/// Model replies per review before we give up on getting a verdict.
const MAX_TURNS: usize = 12;

const SYSTEM_PROMPT: &str = "\
You check a researcher's project folder against their data management plan (DMP).
You get the DMP as maDMP JSON (RDA DMP Common Standard) and a list of files that
just changed in the folder. Decide whether the changes contradict the DMP: for
example data of a kind, format, size or sensitivity the DMP does not describe,
personal data where the DMP says there is none, or files in a format the DMP
rules out. Changes the DMP does not cover at all (notes, scripts, drafts) do not
contradict it.

Use list_dir and read_file to look at the changed files when their names alone
do not settle it. Be frugal: read only what you need. Then call report_verdict
exactly once. Only report a contradiction you can point to in the DMP.";

#[derive(Debug, Deserialize)]
pub struct Verdict {
    pub contradicts: bool,
    /// Where in the DMP the contradiction is, e.g. `dataset[0].personal_data`.
    pub dmp_field: Option<String>,
    pub explanation: String,
}

pub struct Reviewer {
    // genai is async; the rest of damap-helper is not, so it gets a runtime
    // of its own and nothing else has to change.
    runtime: Runtime,
    client: Client,
    model: String,
}

impl Reviewer {
    /// `api_key` may be `None`: local servers usually need none.
    pub fn new(config: &AiConfig, api_key: Option<String>) -> Result<Self> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        Ok(Self {
            runtime,
            client: openai_compatible(&config.url, api_key),
            model: config.model.clone(),
        })
    }

    pub fn model(&self) -> &str {
        &self.model
    }

    /// Asks the model about `changes` under `root`, given the DMP as maDMP JSON.
    pub fn review(&self, root: &Path, dmp: &str, changes: &[Change]) -> Result<Verdict> {
        let tools = Tools::new(root)?;
        let relative = |path: &Path| {
            path.strip_prefix(root)
                .unwrap_or(path)
                .display()
                .to_string()
        };
        let changes: Vec<String> = changes
            .iter()
            .map(|change| match change {
                Change::Created(path) => format!("created: {}", relative(path)),
                Change::Changed(path) => format!("changed: {}", relative(path)),
                Change::Renamed { from, to } => {
                    format!("renamed: {} -> {}", relative(from), relative(to))
                }
                Change::Deleted(path) => format!("deleted: {}", relative(path)),
            })
            .collect();
        let prompt = format!(
            "The DMP:\n```json\n{dmp}\n```\n\nWhat changed in the project folder:\n{}",
            changes.join("\n")
        );
        self.runtime.block_on(self.run(&tools, prompt))
    }

    async fn run(&self, tools: &Tools, prompt: String) -> Result<Verdict> {
        let mut definitions = Tools::definitions();
        definitions.push(verdict_tool());
        let mut request = ChatRequest::new(vec![ChatMessage::user(prompt)])
            .with_system(SYSTEM_PROMPT)
            .with_tools(definitions);
        // Every reply must be a tool call, so the model cannot end the review
        // with prose instead of a verdict. Not every provider honours this,
        // hence the reminder below.
        let options = ChatOptions::default().with_tool_choice(ToolChoice::Required);

        for _ in 0..MAX_TURNS {
            let response = self
                .client
                .exec_chat(&self.model, request.clone(), Some(&options))
                .await
                .with_context(|| format!("the model {} failed", self.model))?;
            let calls: Vec<_> = response.tool_calls().into_iter().cloned().collect();
            // Send the reply back as it came, so provider-specific parts
            // (such as Gemini's thought signatures) survive the round trip.
            request = request.append_message(ChatMessage::assistant(response.content));
            if calls.is_empty() {
                request = request.append_message(ChatMessage::user(
                    "Please finish by calling report_verdict.",
                ));
                continue;
            }
            for call in calls {
                let output = if call.fn_name == "report_verdict" {
                    match serde_json::from_value(call.fn_arguments.clone()) {
                        Ok(verdict) => return Ok(verdict),
                        Err(e) => format!("error: invalid verdict: {e}"),
                    }
                } else {
                    tools
                        .call(&call.fn_name, &call.fn_arguments)
                        .unwrap_or_else(|e| format!("error: {e}"))
                };
                request = request.append_message(ToolResponse::new(call.call_id, output));
            }
        }
        bail!("the model gave no verdict after {MAX_TURNS} replies")
    }
}

fn verdict_tool() -> Tool {
    Tool::new("report_verdict")
        .with_description("Hand in the verdict. Ends the review.")
        .with_schema(json!({
            "type": "object",
            "properties": {
                "contradicts": {
                    "type": "boolean",
                    "description": "Whether the changes contradict the DMP."
                },
                "dmp_field": {
                    "type": "string",
                    "description": "If they do: the DMP field they contradict, e.g. `dataset[0].personal_data`."
                },
                "explanation": {
                    "type": "string",
                    "description": "One or two sentences for the researcher."
                }
            },
            "required": ["contradicts", "explanation"]
        }))
}

/// The models the server at `url` offers, by id.
pub fn list_models(http: &HttpClient, url: &str, api_key: Option<&str>) -> Result<Vec<String>> {
    #[derive(Deserialize)]
    struct Models {
        data: Vec<Model>,
    }
    #[derive(Deserialize)]
    struct Model {
        id: String,
    }

    let url = format!("{}models", base_url(url));
    let mut request = http.get(&url);
    if let Some(key) = api_key {
        request = request.bearer_auth(key);
    }
    let response = request
        .send()
        .with_context(|| format!("could not reach {url}"))?;
    let status = response.status();
    if !status.is_success() {
        bail!("{url} answered {status}; is the URL right, and the API key?");
    }
    let models: Models = response
        .json()
        .with_context(|| format!("{url} does not look like an OpenAI-compatible API"))?;
    let mut ids: Vec<String> = models.data.into_iter().map(|model| model.id).collect();
    ids.sort();
    Ok(ids)
}

/// `url` with a trailing slash: joining `models` or `chat/completions` onto
/// a URL drops its last path segment otherwise.
fn base_url(url: &str) -> String {
    format!("{}/", url.trim_end_matches('/'))
}

/// A genai client that sends every request to the OpenAI-compatible server at `url`.
fn openai_compatible(url: &str, api_key: Option<String>) -> Client {
    let endpoint = Endpoint::from_owned(base_url(url));
    // genai's OpenAI adapter insists on a key; an empty one does no harm.
    let auth = AuthData::from_single(api_key.unwrap_or_default());
    Client::builder()
        .with_service_target_resolver_fn(move |target: ServiceTarget| {
            Ok(ServiceTarget {
                endpoint: endpoint.clone(),
                auth: auth.clone(),
                model: ModelIden::new(AdapterKind::OpenAI, target.model.model_name),
            })
        })
        .build()
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::TcpListener;
    use std::thread;

    use serde_json::Value;

    use super::*;

    /// A request the fake server received: its head (request line and
    /// headers) and its JSON body, `Null` if it had none.
    type Request = (String, Value);

    /// A fake OpenAI-compatible server that answers each request with the
    /// next of `replies` and returns the requests it received.
    fn fake_model(replies: Vec<Value>) -> (String, thread::JoinHandle<Vec<Request>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/v1", listener.local_addr().unwrap());
        let server = thread::spawn(move || {
            let mut requests = Vec::new();
            for reply in replies {
                let (stream, _) = listener.accept().unwrap();
                let mut reader = BufReader::new(stream);
                let mut head = String::new();
                let mut length = 0;
                loop {
                    let mut line = String::new();
                    reader.read_line(&mut line).unwrap();
                    if line == "\r\n" {
                        break;
                    }
                    head.push_str(&line);
                    if let Some((name, value)) = line.split_once(':')
                        && name.eq_ignore_ascii_case("content-length")
                    {
                        length = value.trim().parse().unwrap();
                    }
                }
                let mut body = vec![0; length];
                reader.read_exact(&mut body).unwrap();
                let body = if body.is_empty() {
                    Value::Null
                } else {
                    serde_json::from_slice(&body).unwrap()
                };
                requests.push((head, body));

                let body = reply.to_string();
                write!(
                    reader.get_mut(),
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\
                     Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
                .unwrap();
            }
            requests
        });
        (url, server)
    }

    fn tool_call(name: &str, arguments: Value) -> Value {
        json!({
            "id": "chatcmpl-1",
            "object": "chat.completion",
            "model": "test",
            "choices": [{
                "index": 0,
                "finish_reason": "tool_calls",
                "message": {
                    "role": "assistant",
                    "content": null,
                    "tool_calls": [{
                        "id": format!("call_{name}"),
                        "type": "function",
                        "function": { "name": name, "arguments": arguments.to_string() }
                    }]
                }
            }],
            "usage": { "prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2 }
        })
    }

    #[test]
    fn runs_tools_until_the_model_reports_a_verdict() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir(root.path().join("data")).unwrap();
        fs::write(root.path().join("data/patients.csv"), "name,diagnosis\n").unwrap();
        let (endpoint, server) = fake_model(vec![
            tool_call("read_file", json!({ "path": "data/patients.csv" })),
            tool_call(
                "report_verdict",
                json!({
                    "contradicts": true,
                    "dmp_field": "dataset[0].personal_data",
                    "explanation": "Patient names, but the DMP says no personal data."
                }),
            ),
        ]);
        let reviewer = Reviewer::new(
            &AiConfig {
                url: endpoint,
                model: "test".to_string(),
            },
            Some("sk-test".to_string()),
        )
        .unwrap();

        let verdict = reviewer
            .review(
                root.path(),
                r#"{"dmp": {"dataset": [{"personal_data": "no"}]}}"#,
                &[Change::Created(root.path().join("data/patients.csv"))],
            )
            .unwrap();

        assert!(verdict.contradicts);
        assert_eq!(
            verdict.dmp_field.as_deref(),
            Some("dataset[0].personal_data")
        );
        let requests = server.join().unwrap();
        let (head, body) = &requests[0];
        assert!(head.starts_with("POST /v1/chat/completions "), "{head}");
        assert!(head.contains("authorization: Bearer sk-test"), "{head}");
        let first = body.to_string();
        assert!(first.contains("created: data/patients.csv"), "{first}");
        // The second request carries what read_file found.
        let messages = requests[1].1["messages"].as_array().unwrap();
        let tool_message = messages.iter().find(|m| m["role"] == "tool").unwrap();
        assert_eq!(tool_message["content"], "name,diagnosis\n");
    }

    #[test]
    fn lists_the_models_a_server_offers() {
        let (url, server) = fake_model(vec![json!({
            "object": "list",
            "data": [
                { "id": "qwen3:8b", "object": "model" },
                { "id": "llama3.2", "object": "model" }
            ]
        })]);

        let models = list_models(&HttpClient::new(), &format!("{url}/"), Some("sk-test")).unwrap();

        assert_eq!(models, ["llama3.2", "qwen3:8b"]);
        let (head, _) = &server.join().unwrap()[0];
        assert!(head.starts_with("GET /v1/models "), "{head}");
        assert!(head.contains("authorization: Bearer sk-test"), "{head}");
    }
}
