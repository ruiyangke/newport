use super::View;
use crate::{request_error::RequestError, resources::QueryStatus};
use newport_protocol::desktop::connection::PortsObservation;
use std::sync::Arc;

/// Prepared off the UI thread. Identical cache observations share their view;
/// status updates never serialize discovery records into generic JSON state.
#[derive(Clone, Debug)]
pub struct Reading {
    view: Option<Arc<View>>,
    status: QueryStatus,
}

impl Reading {
    pub fn prepare(
        response: Result<PortsObservation, RequestError>,
        previous: Option<&Self>,
    ) -> Self {
        let observation = match response {
            Ok(observation) => observation,
            Err(error) => return Self::failed(error, previous),
        };
        let stale = observation.stale || observation.error.is_some();
        let view = match observation.data {
            Some(evidence) => {
                if let Some(view) = previous.and_then(Self::view)
                    && view.evidence() == &evidence
                    && view.inventory().fresh != stale
                {
                    Some(view.clone())
                } else {
                    match View::new(evidence, stale) {
                        Ok(view) => Some(Arc::new(view)),
                        Err(error) => return Self::failed(error.to_string().into(), previous),
                    }
                }
            }
            // Withdrawal may fence a changed server scope. Only an explicit
            // retained observation, never fetching alone, keeps old evidence.
            None => None,
        };
        let updated_at = view.as_ref().and(observation.updated_at);
        Self {
            view,
            status: QueryStatus {
                error: observation.error.map(Into::into),
                fetching: observation.fetching,
                stale,
                updated_at,
            },
        }
    }

    fn failed(error: RequestError, previous: Option<&Self>) -> Self {
        let view = previous.and_then(Self::view).map(|view| {
            if view.inventory().fresh {
                Arc::new(view.as_stale())
            } else {
                view.clone()
            }
        });
        let updated_at = view
            .as_ref()
            .and_then(|_| previous.and_then(|reading| reading.status.updated_at));
        Self {
            view,
            status: QueryStatus {
                error: Some(error),
                fetching: false,
                stale: true,
                updated_at,
            },
        }
    }

    pub fn view(&self) -> Option<&Arc<View>> {
        self.view.as_ref()
    }

    pub fn status(&self) -> &QueryStatus {
        &self.status
    }

    pub fn data_changed(&self, previous: Option<&Self>) -> bool {
        match (self.view(), previous.and_then(Self::view)) {
            (Some(a), Some(b)) => !Arc::ptr_eq(a, b),
            (None, None) => false,
            _ => true,
        }
    }
}
