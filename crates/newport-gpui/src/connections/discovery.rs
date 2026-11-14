use super::*;
use newport_client::discovery::Reading;

impl Newport {
    pub(crate) fn complete_connections_reading(
        &mut self,
        server: &str,
        generation: u64,
        completion: DiscoveryCompletion,
    ) -> bool {
        if self.page != Page::Ports || self.server != server {
            return false;
        }
        let scope = self.selected_connection();
        if scope.as_ref() != Some(&completion.scope) {
            return false;
        }
        let previous = self
            .connections_state
            .get(server)
            .and_then(|state| state.reading.as_ref());
        let changed = completion.reading.data_changed(previous);
        if !self.resources.complete_query(
            "ports",
            generation,
            completion.reading.status(),
            completion.reading.view().is_some_and(|view| {
                view.evidence().has_current_inventory() || !view.evidence().retained.is_empty()
            }),
            changed,
        ) {
            return false;
        }
        let state = self.connections_state.entry(server.into()).or_default();
        state.set_scope(completion.scope);
        state.reading = Some(completion.reading);
        state.restored = false;
        true
    }

    pub(crate) fn discover_connections(&mut self, manual: bool) {
        if self.page != Page::Ports || self.server.is_empty() || !self.configuration_available() {
            return;
        }
        let Some(scope) = self.selected_connection() else {
            return;
        };
        let state = self
            .connections_state
            .entry(self.server.clone())
            .or_default();
        if state.set_scope(scope.clone()) {
            self.resources.invalidate("ports");
            self.resources.invalidate("connection-projects");
        }
        if self.resources.get("ports").is_some_and(|row| row.pending) {
            state.refresh_requested |= manual;
            return;
        }
        let manual = manual | std::mem::take(&mut state.refresh_requested);
        let previous = state.reading.clone();
        let Some(generation) = self.resources.begin_refresh("ports", manual) else {
            return;
        };
        let result = desktop_server(&self.server).and_then(|server| {
            self.work
                .try_send(Work {
                    key: "ports".into(),
                    generation,
                    request: request_dispatch::Request::Discovery {
                        server,
                        scope: scope.clone(),
                        refresh: manual,
                        previous: previous.clone(),
                    },
                })
                .map_err(|_| "Too many requests. Try again when an operation finishes.".into())
        });
        if let Err(error) = result {
            self.complete_connections_reading(
                &self.server.clone(),
                generation,
                DiscoveryCompletion {
                    scope,
                    reading: Reading::prepare(Err(error), previous.as_ref()),
                },
            );
        }
    }
}
