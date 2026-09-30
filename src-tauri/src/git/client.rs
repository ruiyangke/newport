//! Async client for the bounded Git channel. A caller discards it after any
//! transport/protocol failure: partially read frames must never be reused.
use super::protocol::{self, Error, Message, Request};
use base64::{engine::general_purpose::STANDARD, Engine};
use serde_json::{json, Value};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use uuid::Uuid;

pub struct Client<S> {
    stream: S,
    methods: Vec<String>,
}
impl<S: AsyncRead + AsyncWrite + Unpin> Client<S> {
    #[cfg(test)]
    pub async fn start(stream: S) -> Result<(Self, Value), Error> {
        Self::start_with_identity(stream, Uuid::new_v4().to_string()).await
    }
    pub async fn start_with_identity(
        mut stream: S,
        client_id: String,
    ) -> Result<(Self, Value), Error> {
        let hello = receive(&mut stream).await?;
        let Message::Hello {
            protocol: name,
            versions,
            limits,
            ..
        } = hello
        else {
            return Err(invalid());
        };
        if name != "newport.git"
            || !versions.contains(&protocol::VERSION)
            || limits["maxFrameBytes"].as_u64() != Some(protocol::MAX_FRAME as u64)
            || limits["maxChunkBytes"].as_u64() != Some(protocol::CHUNK_SIZE as u64)
        {
            return Err(Error::new(
                "UNSUPPORTED_PROTOCOL",
                "Install an agent with compatible Git support.",
            ));
        }
        let id = Uuid::new_v4().to_string();
        send(
            &mut stream,
            &Message::Initialize {
                id: id.clone(),
                version: protocol::VERSION,
                client_id,
                client_version: env!("CARGO_PKG_VERSION").into(),
            },
        )
        .await?;
        let Message::Ready {
            id: reply,
            version,
            capabilities,
        } = receive(&mut stream).await?
        else {
            return Err(invalid());
        };
        if reply != id || version != protocol::VERSION {
            return Err(invalid());
        }
        let methods = capabilities["methods"]
            .as_array()
            .ok_or_else(invalid)?
            .iter()
            .map(|v| v.as_str().map(str::to_owned).ok_or_else(invalid))
            .collect::<Result<Vec<_>, _>>()?;
        Ok((
            Self { stream, methods },
            json!({"capabilities":capabilities,"limits":limits}),
        ))
    }
    pub async fn ping(&mut self) -> Result<(), Error> {
        let nonce = Uuid::new_v4().to_string();
        send(
            &mut self.stream,
            &Message::Ping {
                nonce: nonce.clone(),
            },
        )
        .await?;
        if !matches!(receive(&mut self.stream).await?,Message::Pong { nonce:reply } if reply==nonce)
        {
            return Err(invalid());
        }
        Ok(())
    }
    pub async fn request(&mut self, request: Request) -> Result<Value, Error> {
        let value = serde_json::to_value(&request).map_err(|_| invalid())?;
        let method = value["method"].as_str().ok_or_else(invalid)?.to_owned();
        if !self.methods.contains(&method) {
            return Err(Error::new(
                "UNSUPPORTED_METHOD",
                "The agent does not support this Git method.",
            ));
        }
        let expected_snapshot = match &request {
            Request::Diff { snapshot, .. } => Some(snapshot.as_str()),
            Request::CommitDiff { commit_oid, .. } => Some(commit_oid.as_str()),
            _ => None,
        };
        let id = Uuid::new_v4().to_string();
        send(
            &mut self.stream,
            &Message::Request {
                id: id.clone(),
                method,
                params: value["params"].clone(),
            },
        )
        .await?;
        let mut stream_id = None;
        let mut data = Vec::new();
        let mut last_seq = 0;
        loop {
            match receive(&mut self.stream).await? {
                Message::Begin {
                    id: reply,
                    stream_id: stream,
                    snapshot,
                } if reply == id
                    && stream_id.is_none()
                    && expected_snapshot == Some(snapshot.as_str()) =>
                {
                    stream_id = Some(stream)
                }
                Message::Chunk {
                    id: reply,
                    stream_id: stream,
                    seq,
                    bytes_b64,
                } if reply == id && stream_id.as_ref() == Some(&stream) && seq == last_seq + 1 => {
                    let bytes = STANDARD.decode(&bytes_b64).map_err(|_| invalid())?;
                    if bytes.is_empty()
                        || bytes.len() > protocol::CHUNK_SIZE
                        || data.len() + bytes.len() > protocol::MAX_DIFF
                    {
                        return Err(Error::new(
                            "PROTOCOL_ERROR",
                            "Git stream exceeds its limit.",
                        ));
                    }
                    data.extend(bytes);
                    last_seq = seq;
                    send(
                        &mut self.stream,
                        &Message::Ack {
                            stream_id: stream,
                            seq,
                        },
                    )
                    .await?;
                }
                Message::Response {
                    id: reply,
                    result: Some(value),
                    error: None,
                } if reply == id => {
                    if stream_id.is_some() {
                        if value["lastSeq"].as_u64() != Some(last_seq as u64)
                            || value["bytes"].as_u64() != Some(data.len() as u64)
                        {
                            return Err(invalid());
                        }
                        let diff: Value = serde_json::from_slice(&data).map_err(|_| invalid())?;
                        return Ok(json!({"snapshot":expected_snapshot,"diff":diff}));
                    }
                    if expected_snapshot.is_some() {
                        return Err(invalid());
                    }
                    return Ok(value);
                }
                Message::Response {
                    id: reply,
                    result: None,
                    error: Some(error),
                } if reply == id => return Err(error),
                _ => return Err(invalid()),
            }
        }
    }
}
fn invalid() -> Error {
    Error::new(
        "PROTOCOL_ERROR",
        "Invalid Git agent response. Reconnect before continuing.",
    )
}
async fn receive(stream: &mut (impl AsyncRead + Unpin)) -> Result<Message, Error> {
    let mut header = [0; 5];
    stream
        .read_exact(&mut header)
        .await
        .map_err(|_| Error::transport("Git channel closed or could not be read."))?;
    let size = protocol::payload_length(header).map_err(|_| invalid())?;
    let mut bytes = vec![0; size];
    stream
        .read_exact(&mut bytes)
        .await
        .map_err(|_| Error::transport("Incomplete Git response."))?;
    protocol::decode(&bytes).map_err(|_| invalid())
}
async fn send(stream: &mut (impl AsyncWrite + Unpin), message: &Message) -> Result<(), Error> {
    let frame = protocol::encode(message).map_err(|_| invalid())?;
    stream
        .write_all(&frame)
        .await
        .map_err(|_| Error::transport("Could not send Git request."))?;
    stream
        .flush()
        .await
        .map_err(|_| Error::transport("Could not flush Git request."))
}

#[cfg(test)]
mod tests {
    use super::*;
    async fn handshake(stream: &mut tokio::io::DuplexStream) {
        send(stream,&Message::Hello { protocol:"newport.git".into(),versions:vec![1],instance_id:"test".into(),limits:json!({"maxFrameBytes":protocol::MAX_FRAME,"maxChunkBytes":protocol::CHUNK_SIZE}) }).await.unwrap();
        let Message::Initialize { id, .. } = receive(stream).await.unwrap() else {
            panic!()
        };
        send(
            stream,
            &Message::Ready {
                id,
                version: 1,
                capabilities: json!({"methods":protocol::METHODS}),
            },
        )
        .await
        .unwrap();
    }
    #[tokio::test]
    async fn streamed_diff_requires_order_and_acknowledgement() {
        let (a, mut b) = tokio::io::duplex(128);
        let agent = tokio::spawn(async move {
            handshake(&mut b).await;
            let Message::Request { id, .. } = receive(&mut b).await.unwrap() else {
                panic!()
            };
            send(
                &mut b,
                &Message::Begin {
                    id: id.clone(),
                    stream_id: "s".into(),
                    snapshot: "snap".into(),
                },
            )
            .await
            .unwrap();
            let data = br#"{"files":[],"truncated":false}"#;
            send(
                &mut b,
                &Message::Chunk {
                    id: id.clone(),
                    stream_id: "s".into(),
                    seq: 1,
                    bytes_b64: STANDARD.encode(data),
                },
            )
            .await
            .unwrap();
            assert!(matches!(
                receive(&mut b).await.unwrap(),
                Message::Ack { seq: 1, .. }
            ));
            send(
                &mut b,
                &Message::success(id, json!({"lastSeq":1,"bytes":data.len()})),
            )
            .await
            .unwrap();
        });
        let (mut client, _) = Client::start(a).await.unwrap();
        let result = client
            .request(Request::Diff {
                repo_id: "repo".into(),
                snapshot: "snap".into(),
                entry_id: "entry".into(),
                side: protocol::Side::HeadToIndex,
                context_lines: 3,
            })
            .await
            .unwrap();
        assert_eq!(result["diff"]["truncated"], false);
        agent.await.unwrap();
    }
    #[tokio::test]
    async fn rejects_wrong_response_id() {
        let (a, mut b) = tokio::io::duplex(4096);
        let agent = tokio::spawn(async move {
            handshake(&mut b).await;
            receive(&mut b).await.unwrap();
            send(&mut b, &Message::success("wrong".into(), json!({})))
                .await
                .unwrap();
        });
        let (mut client, _) = Client::start(a).await.unwrap();
        let error = client
            .request(Request::Open {
                path: protocol::Path::new(b"/tmp"),
            })
            .await
            .unwrap_err();
        assert_eq!(error.code, "PROTOCOL_ERROR");
        agent.await.unwrap();
    }
}
