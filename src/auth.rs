// src/auth.rs — bearer token middleware
//
// Rejects any request that doesn't carry:
//   Authorization: Bearer <API_TOKEN>
//
// The /health endpoint is always allowed through so Docker / uptime checks
// work without needing the token.

use axum::{
    body::Body,
    http::{Request, Response, StatusCode},
    response::IntoResponse,
};
use futures::future::BoxFuture;
use std::{
    sync::Arc,
    task::{Context, Poll},
};
use tower::{Layer, Service};

// ── Layer ─────────────────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct BearerAuthLayer {
    token: Arc<String>,
}

impl BearerAuthLayer {
    pub fn new(token: String) -> Self {
        Self { token: Arc::new(token) }
    }
}

impl<S> Layer<S> for BearerAuthLayer {
    type Service = BearerAuthMiddleware<S>;

    fn layer(&self, inner: S) -> Self::Service {
        BearerAuthMiddleware {
            inner,
            token: self.token.clone(),
        }
    }
}

// ── Middleware ────────────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct BearerAuthMiddleware<S> {
    inner: S,
    token: Arc<String>,
}

impl<S> Service<Request<Body>> for BearerAuthMiddleware<S>
where
    S: Service<Request<Body>, Response = Response<Body>> + Send + Clone + 'static,
    S::Future: Send + 'static,
{
    type Response = S::Response;
    type Error = S::Error;
    type Future = BoxFuture<'static, Result<Self::Response, Self::Error>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, req: Request<Body>) -> Self::Future {
        // Always allow /health through — no token needed for healthchecks
        if req.uri().path() == "/health" {
            let fut = self.inner.call(req);
            return Box::pin(async move { fut.await });
        }

        // Extract and validate the bearer token
        let expected = format!("Bearer {}", self.token);
        let authorized = req
            .headers()
            .get(axum::http::header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .map(|v| v == expected)
            .unwrap_or(false);

        if authorized {
            let fut = self.inner.call(req);
            Box::pin(async move { fut.await })
        } else {
            Box::pin(async move {
                Ok((
                    StatusCode::UNAUTHORIZED,
                    axum::http::HeaderMap::new(),
                    "Unauthorized",
                )
                    .into_response())
            })
        }
    }
}
