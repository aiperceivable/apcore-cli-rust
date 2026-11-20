use apcore::{Config, Executor, Registry};
use apcore_cli::{detect_effective_proxy_hazards, detect_proxy_hazards};
use apcore_toolkit::{HTTPProxyRegistryWriter, OpenAPIScanner, ScanOptions};
use serde_json::{json, Value};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;

fn document() -> Value {
    json!({
        "openapi": "3.1.0",
        "info": {"title":"Items", "version":"1.0.0"},
        "components": {"parameters": {
            "ItemId": {"in":"path","name":"item_id","required":true,"schema":{"type":"string"}},
            "Filter": {"in":"query","name":"filter","schema":{"type":"string"}}
        }},
        "paths": {
            "/items/{item_id}": {
                "parameters": [
                    {"$ref":"#/components/parameters/ItemId"},
                    {"in":"query","name":"limit","schema":{"type":"string"}}
                ],
                "get": {
                    "operationId":"get_item",
                    "parameters":[
                        {"in":"query","name":"limit","required":true,"schema":{"type":"integer"}},
                        {"$ref":"#/components/parameters/Filter"}
                    ],
                    "responses":{"200":{"description":"OK"}}
                }
            },
            "/edit/{item_id}": {
                "parameters":[{"$ref":"#/components/parameters/ItemId"}],
                "post": {
                    "operationId":"edit_item",
                    "requestBody":{"content":{"application/json":{"schema":{
                        "type":"object","properties":{"label":{"type":"string"}}
                    }}}},
                    "responses":{"200":{"description":"OK"}}
                }
            },
            "/unsafe": {
                "parameters":[{"$ref":"#/components/parameters/Filter"}],
                "post":{"operationId":"unsafe","responses":{"200":{"description":"OK"}}}
            }
        }
    })
}

async fn capture_request(listener: &TcpListener) -> (String, Vec<u8>) {
    let (socket, _) = listener.accept().await.unwrap();
    let mut reader = BufReader::new(socket);
    let mut request_line = String::new();
    reader.read_line(&mut request_line).await.unwrap();
    let mut content_length = 0;
    loop {
        let mut line = String::new();
        assert_ne!(reader.read_line(&mut line).await.unwrap(), 0);
        if line == "\r\n" {
            break;
        }
        if let Some((name, value)) = line.split_once(':') {
            if name.eq_ignore_ascii_case("content-length") {
                content_length = value.trim().parse().unwrap();
            }
        }
    }
    let mut body = vec![0; content_length];
    reader.read_exact(&mut body).await.unwrap();
    reader
        .get_mut()
        .write_all(b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}")
        .await
        .unwrap();
    (request_line, body)
}

async fn invoke(
    executor: &Executor,
    listener: &TcpListener,
    module_id: &str,
    input: Value,
) -> (String, Vec<u8>) {
    let (output, request) = tokio::time::timeout(Duration::from_secs(5), async {
        tokio::join!(
            executor.call(module_id, input, None, None),
            capture_request(listener)
        )
    })
    .await
    .expect("proxy request must reach the loopback server");
    output.unwrap();
    request
}

#[tokio::test]
async fn test_effective_openapi_parameters_are_validated_and_forwarded() {
    let scanner = OpenAPIScanner::new();
    let document = document();
    let options = ScanOptions::default();
    let legacy = scanner.scan(&document, &options).await.unwrap();
    let legacy_get = legacy
        .iter()
        .find(|module| module.module_id == "get_item")
        .unwrap();
    assert!(legacy_get.input_schema["properties"]
        .get("item_id")
        .is_none());
    assert!(detect_proxy_hazards(&document, &legacy).is_empty());

    let mut modules = scanner
        .scan_with_effective_parameters(&document, &options)
        .await
        .unwrap();
    let hazards = detect_effective_proxy_hazards(&modules);
    assert_eq!(hazards.len(), 1, "{hazards:?}");
    assert_eq!(hazards[0].module_id, "unsafe");
    modules.retain(|module| module.module_id != "unsafe");
    assert!(detect_effective_proxy_hazards(&modules).is_empty());
    let read = modules
        .iter()
        .find(|module| module.module_id == "get_item")
        .unwrap();
    assert_eq!(read.input_schema["required"], json!(["item_id", "limit"]));
    assert_eq!(read.input_schema["properties"]["limit"]["type"], "integer");

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let registry = Registry::new();
    let writer = HTTPProxyRegistryWriter::new(base.clone(), None, 3.0).unwrap();
    assert!(writer
        .write(&modules, &registry)
        .iter()
        .all(|result| result.verified));
    let executor = Executor::new(registry, Config::default());

    let (line, body) = invoke(
        &executor,
        &listener,
        "get_item",
        json!({"item_id":"a/b", "limit":3, "filter":"blue sky"}),
    )
    .await;
    let target = line.split_whitespace().nth(1).unwrap();
    let url = reqwest::Url::parse(&format!("{base}{target}")).unwrap();
    let query: std::collections::BTreeMap<_, _> = url.query_pairs().collect();
    assert_eq!(url.path(), "/items/a%2Fb");
    assert_eq!(query.len(), 2);
    assert_eq!(query["limit"], "3");
    assert_eq!(query["filter"], "blue sky");
    assert!(body.is_empty());

    let (line, body) = invoke(
        &executor,
        &listener,
        "edit_item",
        json!({"item_id":"42", "label":"new"}),
    )
    .await;
    assert_eq!(line, "POST /edit/42 HTTP/1.1\r\n");
    assert_eq!(
        serde_json::from_slice::<Value>(&body).unwrap(),
        json!({"label":"new"})
    );
}

#[tokio::test]
async fn test_effective_array_query_uses_repeated_keys() {
    let document = json!({
        "openapi": "3.1.0",
        "paths": {"/items": {"get": {
            "operationId": "get_items",
            "parameters": [{"name": "tag", "in": "query", "schema": {
                "type": "array", "items": {"type": "string"}
            }}]
        }}}
    });
    let scanner = OpenAPIScanner::new();
    let options = ScanOptions::new();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    for (modules, expected) in [
        (
            scanner.scan(&document, &options).await.unwrap(),
            vec![r#"["red","blue"]"#],
        ),
        (
            scanner
                .scan_with_effective_parameters(&document, &options)
                .await
                .unwrap(),
            vec!["red", "blue"],
        ),
    ] {
        let registry = Registry::new();
        let writer = HTTPProxyRegistryWriter::new(base.clone(), None, 3.0).unwrap();
        assert!(writer
            .write(&modules, &registry)
            .iter()
            .all(|result| result.verified));
        let executor = Executor::new(registry, Config::default());
        let (line, _) = invoke(
            &executor,
            &listener,
            "get_items",
            json!({"tag":["red","blue"]}),
        )
        .await;
        let target = line.split_whitespace().nth(1).unwrap();
        let url = reqwest::Url::parse(&format!("{base}{target}")).unwrap();
        let query: Vec<_> = url
            .query_pairs()
            .filter(|(name, _)| name == "tag")
            .map(|(_, value)| value.into_owned())
            .collect();
        assert_eq!(query, expected);
    }
}

#[tokio::test]
async fn test_effective_required_body_sends_empty_json_for_omitted_optional_fields() {
    let document = json!({
        "openapi": "3.1.0",
        "paths": {"/items": {"post": {
            "operationId": "create_item",
            "requestBody": {
                "required": true,
                "content": {"application/json": {"schema": {
                    "type": "object", "properties": {"name": {"type": "string"}}
                }}}
            }
        }}}
    });
    let scanner = OpenAPIScanner::new();
    let options = ScanOptions::new();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    for (modules, expected_body) in [
        (scanner.scan(&document, &options).await.unwrap(), &b""[..]),
        (
            scanner
                .scan_with_effective_parameters(&document, &options)
                .await
                .unwrap(),
            &b"{}"[..],
        ),
    ] {
        let registry = Registry::new();
        let writer = HTTPProxyRegistryWriter::new(base.clone(), None, 3.0).unwrap();
        assert!(writer
            .write(&modules, &registry)
            .iter()
            .all(|result| result.verified));
        let executor = Executor::new(registry, Config::default());
        let (line, body) = invoke(&executor, &listener, "create_item", json!({})).await;
        assert_eq!(line, "POST /items HTTP/1.1\r\n");
        assert_eq!(body, expected_body);
    }
}
