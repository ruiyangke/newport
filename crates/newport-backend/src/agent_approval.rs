//! Local SSH consent waits, scoped by server so queued reads share the wait.
//! Contains no keys, signatures, or authorization decisions.
use std::{collections::HashMap, sync::LazyLock};
use tokio::sync::watch;
use uuid::Uuid;

type Pending = HashMap<Uuid, usize>;
static PENDING: LazyLock<watch::Sender<Pending>> =
    LazyLock::new(|| watch::channel(HashMap::new()).0);

pub fn subscribe() -> watch::Receiver<Pending> {
    PENDING.subscribe()
}

pub struct Wait(Uuid);
impl Wait {
    pub fn begin(server: Uuid) -> Self {
        PENDING.send_modify(|pending| *pending.entry(server).or_default() += 1);
        Self(server)
    }
}
impl Drop for Wait {
    fn drop(&mut self) {
        PENDING.send_modify(|pending| {
            if let Some(count) = pending.get_mut(&self.0) {
                *count -= 1;
                if *count == 0 {
                    pending.remove(&self.0);
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn waits_are_scoped_and_cancellation_releases_only_its_own_wait() {
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        let state = subscribe();
        let one = Wait::begin(a);
        let two = Wait::begin(a);
        let other = Wait::begin(b);
        assert_eq!(state.borrow().get(&a), Some(&2));
        drop(one);
        assert_eq!(state.borrow().get(&a), Some(&1));
        drop(two);
        assert!(!state.borrow().contains_key(&a));
        assert_eq!(state.borrow().get(&b), Some(&1));
        drop(other);
        assert!(!state.borrow().contains_key(&b));
    }
}
