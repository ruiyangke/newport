use super::*;
use tokio::sync::{mpsc, oneshot};

fn policy() -> Policy {
    Policy {
        stale_after: Duration::from_secs(30),
        retry_after: Duration::from_secs(10),
        timeout: Duration::from_secs(5),
        observation_lease: Duration::from_secs(60),
    }
}

fn fixture() -> (
    Query<u32>,
    mpsc::UnboundedReceiver<oneshot::Sender<Result<u32, String>>>,
) {
    let (calls, receive) = mpsc::unbounded_channel();
    let query = Query::new(policy(), move || {
        let calls = calls.clone();
        async move {
            let (send, receive) = oneshot::channel();
            calls.send(send).unwrap();
            receive.await.unwrap()
        }
    });
    (query, receive)
}

async fn settled(query: &Query<u32>) -> Snapshot<u32> {
    let mut changes = query.subscribe();
    loop {
        let value = query.snapshot();
        if !value.fetching && (value.data.is_some() || value.error.is_some()) {
            return value;
        }
        changes.changed().await.unwrap();
    }
}

#[tokio::test(start_paused = true)]
async fn observers_share_fetch_and_cached_navigation_does_not_refetch() {
    let (query, mut calls) = fixture();
    assert!(calls.try_recv().is_err()); // Creating state does not perform I/O.
    assert!(query.observe(false).fetching);
    let first = calls.recv().await.unwrap();
    for _ in 0..20 {
        query.observe(true);
    }
    tokio::task::yield_now().await;
    assert!(calls.try_recv().is_err());
    first.send(Ok(42)).unwrap();
    let first = settled(&query).await;
    for _ in 0..20 {
        assert_eq!(query.observe(false).data, Some(42));
    }
    tokio::task::yield_now().await;
    assert!(calls.try_recv().is_err());
    assert_eq!(query.snapshot().revision, first.revision);
    assert_eq!(query.snapshot().updated_at, first.updated_at);
}

#[tokio::test(start_paused = true)]
async fn refresh_waiter_cancellation_does_not_own_fetch() {
    let (query, mut calls) = fixture();
    let query = std::sync::Arc::new(query);
    let waiter = tokio::spawn({
        let query = query.clone();
        async move { query.refresh().await }
    });
    let reply = calls.recv().await.unwrap();
    waiter.abort();
    reply.send(Ok(9)).unwrap();
    assert_eq!(settled(&query).await.data, Some(9));
}

#[tokio::test(start_paused = true)]
async fn polling_runs_in_background_until_demand_expires() {
    let (query, mut calls) = fixture();
    query.observe(false);
    calls.recv().await.unwrap().send(Ok(1)).unwrap();
    settled(&query).await;
    tokio::time::advance(Duration::from_secs(30)).await;
    let reply = calls.recv().await.unwrap();
    assert_eq!(query.snapshot().data, Some(1));
    assert!(query.snapshot().stale);
    reply.send(Ok(2)).unwrap();
    settled(&query).await;
    tokio::time::advance(Duration::from_secs(90)).await;
    tokio::task::yield_now().await;
    assert!(calls.try_recv().is_err());
    assert_eq!(query.snapshot().data, Some(2));
    assert!(query.snapshot().stale);
    assert_eq!(query.observe(false).data, Some(2));
    calls.recv().await.unwrap().send(Ok(3)).unwrap();
    assert_eq!(settled(&query).await.data, Some(3));
}

#[tokio::test(start_paused = true)]
async fn failure_keeps_last_success_and_observations_do_not_bypass_retry_delay() {
    let (query, mut calls) = fixture();
    query.observe(false);
    calls.recv().await.unwrap().send(Ok(1)).unwrap();
    let good = settled(&query).await;
    query.observe(true);
    calls
        .recv()
        .await
        .unwrap()
        .send(Err("offline".into()))
        .unwrap();
    let failed = settled(&query).await;
    assert_eq!(failed.data, Some(1));
    assert_eq!(failed.updated_at, good.updated_at);
    assert_eq!(failed.revision, good.revision);
    assert_eq!(failed.error.as_deref(), Some("offline"));
    assert!(failed.stale);
    for _ in 0..20 {
        query.observe(false);
    }
    tokio::task::yield_now().await;
    assert!(calls.try_recv().is_err());
    tokio::time::advance(Duration::from_secs(10)).await;
    calls.recv().await.unwrap().send(Ok(2)).unwrap();
    assert!(settled(&query).await.error.is_none());
}

#[tokio::test(start_paused = true)]
async fn invalidation_cancels_old_request_and_close_hides_scope_data() {
    let (query, mut calls) = fixture();
    query.observe(false);
    let old = calls.recv().await.unwrap();
    query.invalidate();
    let new = calls.recv().await.unwrap();
    tokio::task::yield_now().await;
    assert!(old.send(Ok(111)).is_err());
    new.send(Ok(222)).unwrap();
    assert_eq!(settled(&query).await.data, Some(222));
    query.observe(true);
    let pending = calls.recv().await.unwrap();
    query.close();
    assert!(query.snapshot().data.is_none());
    assert!(query.refresh().await.is_err());
    tokio::task::yield_now().await;
    tokio::task::yield_now().await;
    assert!(pending.send(Ok(333)).is_err());
}

#[tokio::test(start_paused = true)]
async fn slow_query_does_not_block_other_keys_and_timeout_releases_worker() {
    let (slow, mut slow_calls) = fixture();
    let (fast, mut fast_calls) = fixture();
    slow.observe(false);
    let hanging = slow_calls.recv().await.unwrap();
    fast.observe(false);
    fast_calls.recv().await.unwrap().send(Ok(7)).unwrap();
    assert_eq!(settled(&fast).await.data, Some(7));
    tokio::time::advance(Duration::from_secs(5)).await;
    assert_eq!(
        settled(&slow).await.error.as_deref(),
        Some("Query refresh timed out.")
    );
    tokio::task::yield_now().await;
    assert!(hanging.send(Ok(8)).is_err());
    slow.observe(true);
    slow_calls.recv().await.unwrap().send(Ok(9)).unwrap();
    assert_eq!(settled(&slow).await.data, Some(9));
}

#[tokio::test(start_paused = true)]
async fn panicking_loader_settles_as_an_error() {
    let query = Query::<u32>::new(policy(), || async { panic!("fixture panic") });
    query.observe(false);
    assert!(
        settled(&query)
            .await
            .error
            .unwrap()
            .contains("Query worker failed")
    );
}

#[tokio::test(start_paused = true)]
async fn fresh_reads_share_work_and_invalidation_waits_for_a_new_result() {
    let (query, mut calls) = fixture();
    let query = std::sync::Arc::new(query);
    let a = tokio::spawn({
        let query = query.clone();
        async move { query.read().await }
    });
    let first = calls.recv().await.unwrap();
    let b = tokio::spawn({
        let query = query.clone();
        async move { query.read().await }
    });
    tokio::task::yield_now().await;
    assert!(calls.try_recv().is_err());
    first.send(Ok(1)).unwrap();
    assert_eq!(a.await.unwrap(), Ok(1));
    assert_eq!(b.await.unwrap(), Ok(1));
    assert_eq!(query.read().await, Ok(1));
    assert!(calls.try_recv().is_err());
    query.invalidate();
    let fresh = tokio::spawn({
        let query = query.clone();
        async move { query.read().await }
    });
    let next = calls.recv().await.unwrap();
    assert!(!fresh.is_finished());
    next.send(Ok(2)).unwrap();
    assert_eq!(fresh.await.unwrap(), Ok(2));
}

#[tokio::test(start_paused = true)]
async fn parameter_cache_is_bounded_scoped_and_cannot_reopen_after_close() {
    let cache = Registry::<String, u32>::new(2);
    let one = cache
        .get_or_insert("one".into(), policy(), || async { Ok(1) })
        .unwrap();
    let same = cache
        .get_or_insert("one".into(), policy(), || async { Ok(99) })
        .unwrap();
    assert!(std::sync::Arc::ptr_eq(&one, &same));
    assert_eq!(same.read().await, Ok(1));
    let two = cache
        .get_or_insert("two".into(), policy(), || async { Ok(2) })
        .unwrap();
    assert_eq!(two.read().await, Ok(2));
    assert!(
        cache
            .get_or_insert("three".into(), policy(), || async { Ok(3) })
            .is_err()
    );
    cache.invalidate(|key| key == "one");
    assert!(one.snapshot().stale);
    assert!(!two.snapshot().stale);
    drop(one);
    drop(same);
    drop(two);
    tokio::time::advance(Duration::from_secs(61)).await;
    let three = cache
        .get_or_insert("three".into(), policy(), || async { Ok(3) })
        .unwrap();
    assert_eq!(three.read().await, Ok(3));
    cache.close();
    assert!(three.read().await.is_err());
    assert!(
        cache
            .get_or_insert("four".into(), policy(), || async { Ok(4) })
            .is_err()
    );
}

#[tokio::test(start_paused = true)]
async fn structured_errors_survive_query_storage() {
    #[derive(Clone, Debug, PartialEq)]
    struct Error {
        code: &'static str,
        detail: String,
    }
    impl From<String> for Error {
        fn from(detail: String) -> Self {
            Self {
                code: "QUERY",
                detail,
            }
        }
    }
    let expected = Error {
        code: "STALE_SNAPSHOT",
        detail: "Repository changed".into(),
    };
    let error = expected.clone();
    let query = Query::<u32, Error>::new_with_error(policy(), move || {
        let error = error.clone();
        async move { Err(error) }
    });
    assert_eq!(query.read().await, Err(expected));
}
