//! A transport that answers from a script: for the tests of anything whose
//! requests cannot be recorded, above all the OAuth sign-in, which no fixture
//! can hold because every answer in it is a credential.
//!
//! Each [`ScriptTransport::reply`] queues one answer for requests whose URL
//! contains a string; the first matching queue that is not empty answers, in
//! the order the replies were queued. A request nothing answers is a
//! transport error naming its path, so a test that sends something it did not
//! expect fails on that request rather than on a confusing assertion later.
//! Every request is recorded, body included, for the test to read.

use std::collections::VecDeque;
use std::sync::Mutex;

use super::ClientError;
use super::http::{HttpRequest, HttpResponse, Transport};

/// The answers queued for one URL fragment: status and body, in order.
type Queue = (String, VecDeque<(u16, String)>);

/// See the module documentation.
#[derive(Debug, Default)]
pub struct ScriptTransport {
    rules: Mutex<Vec<Queue>>,
    seen: Mutex<Vec<HttpRequest>>,
}

impl ScriptTransport {
    /// An empty script.
    pub fn new() -> Self {
        Self::default()
    }

    /// Queue one answer for the next request whose URL contains `contains`.
    pub fn reply(&self, contains: &str, status: u16, body: &str) -> &Self {
        let mut rules = self.rules.lock().unwrap_or_else(|e| e.into_inner());
        match rules.iter_mut().find(|(c, _)| c == contains) {
            Some((_, queue)) => queue.push_back((status, body.to_string())),
            None => rules.push((
                contains.to_string(),
                VecDeque::from([(status, body.to_string())]),
            )),
        }
        self
    }

    /// Every request made so far, in order.
    pub fn requests(&self) -> Vec<HttpRequest> {
        self.seen.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// How many answers are still queued.
    pub fn remaining(&self) -> usize {
        self.rules
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .map(|(_, q)| q.len())
            .sum()
    }
}

#[async_trait::async_trait]
impl Transport for ScriptTransport {
    async fn execute(&self, request: HttpRequest) -> Result<HttpResponse, ClientError> {
        self.seen
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(request.clone());
        let answer = {
            let mut rules = self.rules.lock().unwrap_or_else(|e| e.into_inner());
            rules
                .iter_mut()
                .find(|(contains, queue)| {
                    request.url.contains(contains.as_str()) && !queue.is_empty()
                })
                .and_then(|(_, queue)| queue.pop_front())
        };
        let Some((status, body)) = answer else {
            return Err(ClientError::Transport(format!(
                "no scripted reply for {} {}",
                request.method, request.path
            )));
        };
        Ok(HttpResponse {
            status,
            body,
            next_page: None,
            ratelimit_remaining: None,
            ratelimit_reset: None,
            retry_after: None,
            etag: None,
            link: None,
            oauth_scopes: None,
        })
    }
}
