use crate::{model::Server, ssh};
use serde::Serialize;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::sync::mpsc;
use uuid::Uuid;

#[derive(Debug, Serialize)]
#[serde(tag = "type", content = "data", rename_all = "camelCase")]
pub enum Event {
    Ready,
    Data(Vec<u8>),
    Exit(Option<u32>),
    Error(String),
}
pub enum Input {
    Data(Vec<u8>),
    Resize(u32, u32),
}
struct Session {
    server: Uuid,
    input: mpsc::Sender<Input>,
    output: Arc<tokio::sync::Mutex<mpsc::Receiver<Event>>>,
    task: tokio::task::JoinHandle<()>,
}
#[derive(Default)]
pub struct Sessions(Mutex<HashMap<Uuid, Session>>);

pub fn size(cols: u32, rows: u32) -> Result<(), String> {
    if !(2..=500).contains(&cols) || !(1..=300).contains(&rows) {
        return Err("Invalid terminal dimensions.".into());
    }
    Ok(())
}
impl Sessions {
    pub fn open(&self, id: Uuid, server: Server, cols: u32, rows: u32) -> Result<(), String> {
        size(cols, rows)?;
        let mut sessions = self.0.lock().unwrap();
        if sessions.contains_key(&id) {
            return Err("Terminal session already exists.".into());
        }
        if sessions.len() >= 8 {
            return Err("Close an existing terminal before opening another.".into());
        }
        let (input, rx) = mpsc::channel(32);
        let (tx, output) = mpsc::channel(32);
        let server_id = server.id;
        let task = tokio::spawn(async move {
            if let Err(error) = ssh::terminal(&server, cols, rows, rx, &tx).await {
                let _ = tx.send(Event::Error(format!("{error:#}"))).await;
            }
        });
        sessions.insert(
            id,
            Session {
                server: server_id,
                input,
                output: Arc::new(tokio::sync::Mutex::new(output)),
                task,
            },
        );
        Ok(())
    }
    pub async fn read(&self, id: Uuid) -> Result<Option<Event>, String> {
        let output = self
            .0
            .lock()
            .unwrap()
            .get(&id)
            .map(|s| s.output.clone())
            .ok_or("Terminal is closed.")?;
        let mut output = output
            .try_lock()
            .map_err(|_| "Terminal already has a reader.")?;
        match tokio::time::timeout(Duration::from_millis(500), output.recv()).await {
            Ok(Some(event)) => Ok(Some(event)),
            Ok(None) => Ok(Some(Event::Exit(None))),
            Err(_) => Ok(None),
        }
    }
    pub fn send(&self, id: Uuid, input: Input) -> Result<(), String> {
        let sessions = self.0.lock().unwrap();
        let session = sessions.get(&id).ok_or("Terminal is closed.")?;
        session
            .input
            .try_send(input)
            .map_err(|_| "Terminal input is busy or disconnected. Reconnect if needed.".into())
    }
    pub async fn close(&self, id: Uuid) {
        let session = self.0.lock().unwrap().remove(&id);
        if let Some(session) = session {
            session.task.abort();
            let _ = session.task.await;
        }
    }
    pub async fn close_server(&self, server: Option<Uuid>) {
        let ids: Vec<_> = self
            .0
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, s)| server.is_none_or(|id| id == s.server))
            .map(|(id, _)| *id)
            .collect();
        for id in ids {
            self.close(id).await;
        }
    }
}
impl Drop for Sessions {
    fn drop(&mut self) {
        for (_, session) in self.0.get_mut().unwrap().drain() {
            session.task.abort();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> Server {
        Server {
            id: Uuid::new_v4(),
            name: "Terminal fixture".into(),
            ssh_user: "fixture".into(),
            ssh_host: "127.0.0.1".into(),
            ssh_port: std::env::var("PORTHOP_TEST_SSH_PORT")
                .unwrap()
                .parse()
                .unwrap(),
            identity_file: Some(std::env::var("PORTHOP_TEST_IDENTITY").unwrap()),
            agent_source: None,
            agent_key_fingerprint: None,
            auth_method: crate::model::AuthMethod::PublicKey,
            clipboard_enabled: false,
        }
    }
    async fn until(sessions: &Sessions, id: Uuid, needle: &str) {
        tokio::time::timeout(Duration::from_secs(8), async {
            let mut text = String::new();
            loop {
                match sessions.read(id).await.unwrap() {
                    Some(Event::Data(bytes)) => {
                        text.push_str(&String::from_utf8_lossy(&bytes));
                        if text.contains(needle) {
                            return;
                        }
                    }
                    Some(Event::Error(error)) => panic!("{error}"),
                    Some(Event::Exit(code)) => panic!("Unexpected exit {code:?}: {text}"),
                    _ => {}
                }
            }
        })
        .await
        .unwrap();
    }
    #[tokio::test]
    #[ignore = "Run with scripts/test-ssh.sh; runs a real Bash PTY in an isolated HOME"]
    async fn interactive_pty_input_resize_interrupt_exit_and_close() {
        let sessions = Sessions::default();
        let server = fixture();
        let id = Uuid::new_v4();
        sessions.open(id, server.clone(), 80, 24).unwrap();
        until(&sessions, id, "early shell output").await;
        until(&sessions, id, "fixture> ").await;
        sessions
            .send(
                id,
                Input::Data(b"export PORTHOP_VALUE=persistent\r".to_vec()),
            )
            .unwrap();
        until(&sessions, id, "fixture> ").await;
        sessions
            .send(
                id,
                Input::Data(b"printf 'value=%s\\n' \"$PORTHOP_VALUE\"\r".to_vec()),
            )
            .unwrap();
        until(&sessions, id, "value=persistent").await;
        sessions.send(id, Input::Resize(100, 40)).unwrap();
        sessions
            .send(id, Input::Data(b"stty size\r".to_vec()))
            .unwrap();
        until(&sessions, id, "40 100").await;
        sessions
            .send(id, Input::Data(b"sleep 60\r".to_vec()))
            .unwrap();
        tokio::time::sleep(Duration::from_millis(150)).await;
        sessions.send(id, Input::Data(vec![3])).unwrap();
        until(&sessions, id, "^C").await;
        sessions
            .send(id, Input::Data(b"exit 7\r".to_vec()))
            .unwrap();
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                match sessions.read(id).await.unwrap() {
                    Some(Event::Exit(code)) => {
                        assert_eq!(code, Some(7));
                        break;
                    }
                    Some(Event::Data(_)) => {}
                    Some(Event::Error(error)) => panic!("{error}"),
                    _ => {}
                }
            }
        })
        .await
        .unwrap();
        sessions.close(id).await;
        assert!(sessions.read(id).await.is_err());
        let rejected = Uuid::new_v4();
        sessions.open(rejected, server.clone(), 13, 24).unwrap();
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if let Some(Event::Error(error)) = sessions.read(rejected).await.unwrap() {
                    assert!(error.contains("refused"));
                    break;
                }
            }
        })
        .await
        .unwrap();
        sessions.close_server(Some(server.id)).await;
        assert!(sessions.0.lock().unwrap().is_empty());
        let cancelled = Uuid::new_v4();
        sessions.open(cancelled, server, 80, 24).unwrap();
        sessions.close(cancelled).await;
        assert!(sessions.0.lock().unwrap().is_empty());
    }
}
