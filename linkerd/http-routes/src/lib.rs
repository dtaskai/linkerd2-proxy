#![deny(rust_2018_idioms, clippy::disallowed_methods, clippy::disallowed_types)]
#![forbid(unsafe_code)]

pub mod filter;
mod r#match;
//pub mod service;

use self::r#match::{HostMatch, PathMatch, RequestMatch};
pub use self::r#match::{MatchHost, MatchRequest};
use std::sync::Arc;

/// Holds all routes that may be considered for a given request.
///
/// HttpRoutes are selected by finding the route that matches "most". When multiple
/// routes match equivalently, the first one is used.
#[derive(Clone, Debug, Hash, PartialEq, Eq)]
pub struct HttpRoutes<T>(pub Arc<[HttpRoute<T>]>);

///
#[derive(Clone, Debug, Default, Hash, PartialEq, Eq)]
pub struct HttpRoute<T> {
    pub hosts: Vec<MatchHost>,
    pub rules: Vec<HttpRule<T>>,
}

#[derive(Clone, Debug, Default, Hash, PartialEq, Eq)]
pub struct HttpRule<T> {
    pub matches: Vec<MatchRequest>,
    pub policy: T,
}

#[derive(Clone, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub struct HttpRouteMatch {
    host: Option<HostMatch>,
    request: RequestMatch,
}

// === impl HttpRoutes ===

impl<T> Default for HttpRoutes<T> {
    fn default() -> Self {
        Self(Arc::new([]))
    }
}

impl<T> HttpRoutes<T> {
    pub fn find<B>(&self, req: &http::Request<B>) -> Option<(HttpRouteMatch, &T)> {
        self.0
            .iter()
            .filter_map(|rt| rt.find(req))
            // This is roughly equivalent to `max_by(...)` but we want to ensure
            // that the first match wins.
            .reduce(|(m0, t0), (m, t)| if m0 < m { (m, t) } else { (m0, t0) })
    }
}

// === impl HttpRoute ===

impl<T> HttpRoute<T> {
    fn find<B>(&self, req: &http::Request<B>) -> Option<(HttpRouteMatch, &T)> {
        let host = if self.hosts.is_empty() {
            None
        } else {
            let uri = req.uri();
            let hm = self
                .hosts
                .iter()
                .filter_map(|a| a.summarize_match(uri))
                .max()?;
            Some(hm)
        };

        let (request, policy) = self
            .rules
            .iter()
            .filter_map(|rule| {
                // If there are no matches in the list, then the rule has an
                // implicit default match.
                if rule.matches.is_empty() {
                    return Some((RequestMatch::default(), &rule.policy));
                }
                // Find the best match to compare against other rules/routes (if
                // any apply). The order/precedence of matche is not relevant.
                let m = rule
                    .matches
                    .iter()
                    .filter_map(|m| m.summarize_match(req))
                    .max()?;
                Some((m, &rule.policy))
            })
            // This is roughly equivalent to `max_by(...)` but we want to ensure
            // that the first match wins.
            .reduce(|(m0, p0), (m, p)| if m0 < m { (m, p) } else { (m0, p0) })?;

        Some((HttpRouteMatch { host, request }, policy))
    }
}

#[cfg(test)]
mod tests {
    use super::{r#match::*, *};

    #[derive(Copy, Clone, Debug, PartialEq, Eq)]
    pub enum Policy {
        Expected,
        Unexpected,
    }

    impl Default for Policy {
        fn default() -> Self {
            Self::Unexpected
        }
    }

    /// Given two equivalent routes, choose the explicit hostname match and not
    /// the wildcard.
    #[test]
    fn hostname_precedence() {
        let rts = HttpRoutes(
            vec![
                HttpRoute {
                    hosts: vec!["*.example.com".parse().unwrap()],
                    rules: vec![HttpRule {
                        matches: vec![MatchRequest {
                            path: Some(MatchPath::Exact("/foo".to_string())),
                            ..MatchRequest::default()
                        }],
                        ..HttpRule::default()
                    }],
                },
                HttpRoute {
                    hosts: vec!["foo.example.com".parse().unwrap()],
                    rules: vec![HttpRule {
                        matches: vec![MatchRequest {
                            path: Some(MatchPath::Exact("/foo".to_string())),
                            ..MatchRequest::default()
                        }],
                        policy: Policy::Expected,
                    }],
                },
            ]
            .into(),
        );

        let req = http::Request::builder()
            .uri("http://foo.example.com/foo")
            .body(())
            .unwrap();
        let (_, policy) = rts.find(&req).expect("must match");
        assert_eq!(*policy, Policy::Expected, "incorrect rule matched");
    }

    #[test]
    fn path_length_precedence() {
        // Given two equivalent routes, choose the longer path match.
        let rts = HttpRoutes(
            vec![
                HttpRoute {
                    rules: vec![HttpRule {
                        matches: vec![MatchRequest {
                            path: Some(MatchPath::Prefix("/foo".to_string())),
                            ..MatchRequest::default()
                        }],
                        ..HttpRule::default()
                    }],
                    ..HttpRoute::default()
                },
                HttpRoute {
                    rules: vec![HttpRule {
                        matches: vec![MatchRequest {
                            path: Some(MatchPath::Exact("/foo/bar".to_string())),
                            ..MatchRequest::default()
                        }],
                        policy: Policy::Expected,
                    }],
                    ..HttpRoute::default()
                },
            ]
            .into(),
        );

        let req = http::Request::builder()
            .uri("http://foo.example.com/foo/bar")
            .body(())
            .unwrap();
        let (_, policy) = rts.find(&req).expect("must match");
        assert_eq!(*policy, Policy::Expected, "incorrect rule matched");
    }

    /// Given two routes with header matches, use the one that matches more
    /// headers.
    #[test]
    fn header_count_precedence() {
        let rts = HttpRoutes(
            vec![
                HttpRoute {
                    rules: vec![HttpRule {
                        matches: vec![MatchRequest {
                            headers: vec![
                                MatchHeader::Exact(
                                    "x-foo".parse().unwrap(),
                                    "bar".parse().unwrap(),
                                ),
                                MatchHeader::Exact(
                                    "x-baz".parse().unwrap(),
                                    "qux".parse().unwrap(),
                                ),
                            ],
                            ..MatchRequest::default()
                        }],
                        ..HttpRule::default()
                    }],
                    ..HttpRoute::default()
                },
                HttpRoute {
                    rules: vec![HttpRule {
                        matches: vec![MatchRequest {
                            headers: vec![
                                MatchHeader::Exact(
                                    "x-foo".parse().unwrap(),
                                    "bar".parse().unwrap(),
                                ),
                                MatchHeader::Exact(
                                    "x-baz".parse().unwrap(),
                                    "qux".parse().unwrap(),
                                ),
                                MatchHeader::Exact(
                                    "x-biz".parse().unwrap(),
                                    "qyx".parse().unwrap(),
                                ),
                            ],
                            ..MatchRequest::default()
                        }],
                        policy: Policy::Expected,
                    }],
                    ..HttpRoute::default()
                },
            ]
            .into(),
        );

        let req = http::Request::builder()
            .uri("http://www.example.com")
            .header("x-foo", "bar")
            .header("x-baz", "qux")
            .header("x-biz", "qyx")
            .body(())
            .unwrap();
        let (_, policy) = rts.find(&req).expect("must match");
        assert_eq!(*policy, Policy::Expected, "incorrect rule matched");
    }

    /// Given two routes with header matches, use the one that matches more
    /// headers.
    #[test]
    fn first_identical_wins() {
        let rts = HttpRoutes(
            vec![
                HttpRoute {
                    rules: vec![
                        HttpRule {
                            policy: Policy::Expected,
                            ..HttpRule::default()
                        },
                        // Redundant rule.
                        HttpRule::default(),
                    ],
                    ..HttpRoute::default()
                },
                // Redundant unlabeled route.
                HttpRoute {
                    rules: vec![HttpRule::default()],
                    ..HttpRoute::default()
                },
            ]
            .into(),
        );

        let (_, policy) = rts
            .find(&http::Request::builder().body(()).unwrap())
            .expect("must match");
        assert_eq!(*policy, Policy::Expected, "incorrect rule matched");
    }
}
