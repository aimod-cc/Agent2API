use serde_json::json;

use super::{parser, request, Profile, CLOSE, OPEN};

fn test_profile() -> Profile {
    Profile::from_model(&json!({
        "id": "glm-5.3-flash",
        "toolEmulation": {
            "protocol": "tagged_json_v1",
            "maxPromptChars": 190000
        }
    }))
    .unwrap()
}

fn sample_body() -> serde_json::Value {
    json!({
        "model": "glm-5.3-flash",
        "messages": [
            { "role": "user", "content": "看下 node 版本" }
        ],
        "tools": [
            {
                "type": "function",
                "function": {
                    "name": "Bash",
                    "description": "Execute a shell command",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "command": { "type": "string" },
                            "description": { "type": "string" }
                        },
                        "required": ["command"],
                        "additionalProperties": false
                    }
                }
            }
        ]
    })
}

#[test]
fn prompt_keeps_natural_key_order_nonce_calls_name_arguments() {
    let prepared = request::prepare(&sample_body(), &test_profile())
        .unwrap()
        .unwrap();
    let ctx = prepared.context.as_ref().unwrap();
    let prompt = prepared.body["messages"][0]["content"].as_str().unwrap();

    let expected_template = format!(
        r#"{OPEN}{{"nonce":"{}","calls":[{{"name":"<declared_name>","arguments":{{}}}}]}}{CLOSE}"#,
        ctx.nonce
    );
    assert!(
        prompt.contains(&expected_template),
        "template should place nonce before calls and name before arguments:\n{prompt}"
    );
    assert!(
        prompt.contains(r#"[{"name":"Bash","description":"Execute a shell command","parameters":"#),
        "tool schema list should place name before description and parameters"
    );
    assert!(
        prompt.contains(r#"[{"role":"user","content":"看下 node 版本"}]"#),
        "conversation transcript should place role before content"
    );
}

#[test]
fn parser_accepts_inner_code_fence_wrapper_fields_and_stringified_arguments() {
    let prepared = request::prepare(&sample_body(), &test_profile())
        .unwrap()
        .unwrap();
    let ctx = prepared.context.unwrap();

    // 1. Markdown fence inside tags + natural key order
    let fenced = format!(
        "{OPEN}\n```json\n{{\"nonce\":\"{}\",\"calls\":[{{\"name\":\"Bash\",\"arguments\":{{\"command\":\"node -v\",\"description\":\"Check node version\"}}}}]}}\n```\n{CLOSE}",
        ctx.nonce
    );
    let out = parser::parse(&fenced, &ctx).unwrap();
    match out {
        parser::Output::Calls { calls, .. } => {
            assert_eq!(calls.len(), 1);
            assert_eq!(calls[0]["function"]["name"], "Bash");
            let args: serde_json::Value =
                serde_json::from_str(calls[0]["function"]["arguments"].as_str().unwrap()).unwrap();
            assert_eq!(args["command"], "node -v");
            assert_eq!(args["description"], "Check node version");
        }
        parser::Output::Text(_) => panic!("expected tool calls"),
    }

    // 2. OpenAI-style wrapper with id/type/function + stringified arguments
    let wrapped = format!(
        "{OPEN}{{\"nonce\":\"{}\",\"calls\":[{{\"id\":\"call_1\",\"type\":\"function\",\"function\":{{\"name\":\"Bash\",\"arguments\":\"{{\\\"command\\\":\\\"node -v\\\"}}\"}}}}]}}{CLOSE}",
        ctx.nonce
    );
    let out2 = parser::parse(&wrapped, &ctx).unwrap();
    match out2 {
        parser::Output::Calls { calls, .. } => {
            assert_eq!(calls.len(), 1);
            assert_eq!(calls[0]["function"]["name"], "Bash");
        }
        parser::Output::Text(_) => panic!("expected tool calls"),
    }
}

#[test]
fn parser_still_rejects_malformed_json_and_unknown_sibling_fields() {
    let prepared = request::prepare(&sample_body(), &test_profile())
        .unwrap()
        .unwrap();
    let ctx = prepared.context.unwrap();

    // Exact malformed brace pattern previously produced by glm-5.3-flash
    let broken_braces = format!(
        "{OPEN}{{\"calls\":[{{\"arguments\":{{\"command\":\"node --version\"}},\"description\":\"Check Node\"}},\"name\":\"Bash\"]],\"nonce\":\"{}\"}}{CLOSE}",
        ctx.nonce
    );
    assert!(parser::parse(&broken_braces, &ctx).is_err());

    // Valid JSON but parameter leaked as sibling field outside arguments must still fail
    let leaked_sibling = format!(
        "{OPEN}{{\"nonce\":\"{}\",\"calls\":[{{\"name\":\"Bash\",\"arguments\":{{\"command\":\"node -v\"}},\"description\":\"Check Node\"}}]}}{CLOSE}",
        ctx.nonce
    );
    assert!(parser::parse(&leaked_sibling, &ctx).is_err());

    // Wrong nonce must still fail
    let wrong_nonce = format!(
        "{OPEN}{{\"nonce\":\"0000\",\"calls\":[{{\"name\":\"Bash\",\"arguments\":{{\"command\":\"node -v\"}}}}]}}{CLOSE}"
    );
    assert!(parser::parse(&wrong_nonce, &ctx).is_err());
}
