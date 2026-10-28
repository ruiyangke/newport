//! Resolve candidates before adapting their server-coordinate edits to GPUI.
//! The toolkit has no selected-item resolve hook. Bound parallel work and the
//! total delay; unresolved candidates retain their original, usable contents.
use super::*;
use futures::{StreamExt, stream};

pub(super) async fn resolve(
    connection: Connection,
    items: Vec<lsp_types::CompletionItem>,
    deadline: impl std::future::Future<Output = ()>,
) -> Vec<lsp_types::CompletionItem> {
    let mut resolved = items.clone();
    let mut requests = stream::iter(items.into_iter().enumerate())
        .map(|(index, item)| {
            let connection = connection.clone();
            async move {
                let result = connection
                    .request::<lsp_types::request::ResolveCompletionItem>(item.clone())
                    .await;
                (index, item, result)
            }
        })
        .buffer_unordered(8);
    let work = async {
        while let Some((index, original, result)) = requests.next().await {
            if let Ok(item) = result
                && item.label == original.label
            {
                // Only these properties were negotiated as lazily resolved.
                // Labels, filtering and primary edits must remain stable.
                let target = &mut resolved[index];
                if item.detail.is_some() {
                    target.detail = item.detail;
                }
                if item.documentation.is_some() {
                    target.documentation = item.documentation;
                }
                if item.additional_text_edits.is_some() {
                    target.additional_text_edits = item.additional_text_edits;
                }
            }
        }
    };
    futures_lite::future::race(work, deadline).await;
    // Drop in-flight requests here: their guards send $/cancelRequest.
    resolved
}

#[cfg(test)]
mod tests {
    use super::super::{Duration, Value, test_support};
    use super::resolve;
    use serde_json::json;
    use std::io::Write;
    fn send(writer: &mut std::io::PipeWriter, value: Value) {
        let body = serde_json::to_vec(&value).unwrap();
        let mut message = format!("Content-Length: {}\r\n\r\n", body.len()).into_bytes();
        message.extend_from_slice(&body);
        let mut frame = vec![newport_platform::ipc::protocol::REMOTE_STREAM_DATA];
        frame.extend_from_slice(&(message.len() as u32).to_be_bytes());
        frame.extend_from_slice(&message);
        writer.write_all(&frame).unwrap();
    }
    #[test]
    fn resolution_preserves_original_identity_and_adds_deferred_imports() {
        let (connection, mut writer, requests) = test_support::connection();
        let server = std::thread::spawn(move || {
            let request = requests.recv_timeout(Duration::from_secs(2)).unwrap();
            assert_eq!(request["method"], "completionItem/resolve");
            assert_eq!(request["params"]["data"]["token"], 7);
            assert_eq!(
                request["params"]["textEdit"]["range"]["end"]["character"],
                3
            );
            let mut result = request["params"].clone();
            result["detail"] = json!("module::print");
            result["textEdit"]["newText"] = json!("unadvertised replacement");
            result["additionalTextEdits"] = json!([{"range":{"start":{"line":0,"character":0},"end":{"line":0,"character":0}},"newText":"use module::print;\n"}]);
            send(
                &mut writer,
                json!({"jsonrpc":"2.0","id":request["id"],"result":result}),
            );
            // Keep the fake server alive until the client's last handle drops.
            while requests.recv_timeout(Duration::from_secs(1)).is_ok() {}
        });
        let item = serde_json::from_value(json!({"label":"print","data":{"token":7},"textEdit":{"range":{"start":{"line":1,"character":0},"end":{"line":1,"character":3}},"newText":"print"}})).unwrap();
        let result =
            futures_lite::future::block_on(resolve(connection, vec![item], std::future::pending()));
        assert_eq!(result[0].detail.as_deref(), Some("module::print"));
        assert_eq!(result[0].additional_text_edits.as_ref().unwrap().len(), 1);
        assert!(
            matches!(&result[0].text_edit, Some(lsp_types::CompletionTextEdit::Edit(edit)) if edit.new_text == "print")
        );
        server.join().unwrap();
    }
    #[test]
    fn deadline_cancels_only_the_bounded_in_flight_resolutions() {
        let (connection, _writer, messages) = test_support::connection();
        let (expire, expired) = async_channel::bounded(1);
        let server = std::thread::spawn(move || {
            for _ in 0..8 {
                assert_eq!(
                    messages.recv_timeout(Duration::from_secs(2)).unwrap()["method"],
                    "completionItem/resolve"
                );
            }
            expire.send_blocking(()).unwrap();
            for _ in 0..8 {
                assert_eq!(
                    messages.recv_timeout(Duration::from_secs(2)).unwrap()["method"],
                    "$/cancelRequest"
                );
            }
        });
        let items = (0..100)
            .map(|i| lsp_types::CompletionItem {
                label: i.to_string(),
                ..Default::default()
            })
            .collect();
        let result = futures_lite::future::block_on(resolve(connection, items, async {
            expired.recv().await.unwrap();
        }));
        assert_eq!(result.len(), 100);
        server.join().unwrap();
    }
}
