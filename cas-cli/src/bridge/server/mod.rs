//! Long-lived local helper server for external orchestration tools.
//!
//! This is intended to replace repeated `cas ... --json` subprocess calls with a single
//! long-running process that reads/writes CAS stores directly.

mod factory;
mod http;
mod routes;
pub(crate) mod session;
mod sse;
pub(crate) mod types;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Context, Result};
use tiny_http::{HTTPVersion, Header, Method, Response, Server};

use crate::bridge::server::factory::handle_factory_start;
use crate::bridge::server::http::{error_response, json_response, require_auth, url_decode};
use crate::bridge::server::routes::handle_session_routes;
use crate::bridge::server::sse::handle_session_events_sse_request;
use crate::bridge::server::types::{
    Health, ServeInfo, SessionListJson, StartFactoryRequest, session_json,
};
use crate::cli::bridge::ServeArgs;
use crate::cli::{Cli, ListArgs};

const STATUS_CACHE_TTL_MS: u64 = 250;

pub fn serve(args: &ServeArgs, cli: &Cli) -> Result<()> {
    use std::io::Write;

    let token = if args.no_auth {
        None
    } else {
        Some(
            args.token
                .clone()
                .unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
        )
    };

    let addr = format!("{}:{}", args.bind, args.port);
    let server = Server::http(&addr).map_err(|e| anyhow::anyhow!("Failed to bind {addr}: {e}"))?;

    let port = server
        .server_addr()
        .to_ip()
        .map(|a| a.port())
        .unwrap_or(args.port);

    let base_url = format!("http://{}:{}", args.bind, port);
    let info = ServeInfo {
        schema_version: 1,
        bind: args.bind.clone(),
        port,
        base_url: base_url.clone(),
        cas_root: args
            .cas_root
            .as_ref()
            .map(|p| p.to_string_lossy().to_string()),
        token: token.clone().filter(|_| !args.no_auth),
        auth_enabled: !args.no_auth,
    };

    if cli.json {
        // One line JSON so callers can read a single line from stdout.
        println!("{}", serde_json::to_string(&info)?);
        let _ = std::io::stdout().flush();
    } else {
        println!("CAS Bridge Server");
        println!("  Base URL: {base_url}");
        if let Some(ref t) = token {
            println!("  Token:    {t}");
        } else {
            println!("  Token:    (disabled)");
        }
    }

    let shutdown = Arc::new(AtomicBool::new(false));
    let cors = args.cors_allow_origin.as_deref().map(|s| s.to_string());
    let fallback_cas_root = args.cas_root.clone();

    // Cache only the expensive aggregated status endpoint.
    let status_cache: std::sync::Mutex<
        std::collections::HashMap<String, (std::time::Instant, Vec<u8>)>,
    > = std::sync::Mutex::new(std::collections::HashMap::new());

    for mut req in server.incoming_requests() {
        if shutdown.load(Ordering::Relaxed) {
            break;
        }

        // CORS preflight
        if req.method() == &Method::Options {
            let resp = Response::empty(204);
            let resp = if let Some(ref origin) = cors {
                let mut resp = resp;
                resp.add_header(
                    Header::from_bytes("Access-Control-Allow-Origin", origin.as_bytes()).unwrap(),
                );
                resp.add_header(
                    Header::from_bytes(
                        "Access-Control-Allow-Headers",
                        "authorization, content-type",
                    )
                    .unwrap(),
                );
                resp.add_header(
                    Header::from_bytes("Access-Control-Allow-Methods", "GET, POST, OPTIONS")
                        .unwrap(),
                );
                resp
            } else {
                resp
            };
            let _ = req.respond(resp);
            continue;
        }

        if let Err(code) = require_auth(&req, token.as_deref(), args.no_auth) {
            let _ = req.respond(error_response(
                tiny_http::StatusCode(401),
                code,
                "Unauthorized",
                cors.as_deref(),
            ));
            continue;
        }

        let url = req.url().to_string();
        let (path_str, query_str) = url.split_once('?').unwrap_or((&url, ""));
        // Keep &str for normal routing, but also create owned copies for SSE threads.
        let path = path_str.to_string();
        let query = query_str.to_string();

        // SSE session events stream is long-lived, so it must not block the main accept loop.
        // Handle it on a dedicated thread and continue accepting other requests.
        if req.method() == &Method::Get
            && path_str.starts_with("/v1/sessions/")
            && path_str.ends_with("/events")
        {
            if req.http_version() == &HTTPVersion(1, 0) {
                let _ = req.respond(error_response(
                    tiny_http::StatusCode(505),
                    "http_version_not_supported",
                    "SSE requires HTTP/1.1",
                    cors.as_deref(),
                ));
                continue;
            }

            let cors2 = cors.clone();
            let fallback2 = fallback_cas_root.clone();
            let shutdown2 = shutdown.clone();
            std::thread::spawn(move || {
                handle_session_events_sse_request(req, path, query, cors2, fallback2, shutdown2);
            });
            continue;
        }

        let respond = match (req.method(), path_str) {
            (&Method::Get, "/v1/health") => Ok(json_response(
                tiny_http::StatusCode(200),
                &Health {
                    schema_version: 1,
                    ok: true,
                },
                cors.as_deref(),
            )),

            (&Method::Post, "/v1/shutdown") => {
                shutdown.store(true, Ordering::Relaxed);
                server.unblock();
                Ok(json_response(
                    tiny_http::StatusCode(200),
                    &Health {
                        schema_version: 1,
                        ok: true,
                    },
                    cors.as_deref(),
                ))
            }

            (&Method::Post, "/v1/factory/start") => {
                let mut body = String::new();
                req.as_reader().read_to_string(&mut body)?;
                let start: StartFactoryRequest = serde_json::from_str(&body)
                    .with_context(|| "Invalid JSON body for factory start request")?;
                handle_factory_start(start, cors.as_deref())
            }

            (&Method::Get, "/v1/sessions") => {
                let mut sessions = crate::ui::factory::SessionManager::new().list_sessions()?;

                // Parse query params
                let mut filters = ListArgs::default();
                for pair in query_str.split('&').filter(|s| !s.is_empty()) {
                    let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
                    let v = url_decode(v);
                    match k {
                        "name" if !v.is_empty() => filters.name = Some(v),
                        "project_dir" if !v.is_empty() => {
                            filters.project_dir = Some(std::path::PathBuf::from(v))
                        }
                        "attachable_only" => filters.attachable_only = v == "1" || v == "true",
                        "running_only" => filters.running_only = v == "1" || v == "true",
                        _ => {}
                    }
                }

                if filters.running_only {
                    sessions.retain(|s| s.is_running);
                }
                if filters.attachable_only {
                    sessions.retain(|s| s.can_attach());
                }
                if let Some(ref name) = filters.name {
                    sessions.retain(|s| &s.name == name);
                }
                if let Some(ref project_dir) = filters.project_dir {
                    let project_dir = project_dir.to_string_lossy();
                    sessions.retain(|s| {
                        s.metadata
                            .project_dir
                            .as_ref()
                            .map(|p| p == project_dir.as_ref())
                            .unwrap_or(false)
                    });
                }

                let out = SessionListJson {
                    schema_version: 1,
                    sessions: sessions.iter().map(session_json).collect(),
                };
                Ok(json_response(
                    tiny_http::StatusCode(200),
                    &out,
                    cors.as_deref(),
                ))
            }

            _ if path_str.starts_with("/v1/sessions/") => handle_session_routes(
                &mut req,
                path_str,
                query_str,
                cors.as_deref(),
                fallback_cas_root.as_deref(),
                &status_cache,
                STATUS_CACHE_TTL_MS,
            ),

            _ => Ok(error_response(
                tiny_http::StatusCode(404),
                "not_found",
                "Unknown route",
                cors.as_deref(),
            )),
        };

        match respond {
            Ok(resp) => {
                let _ = req.respond(resp);
            }
            Err(e) => {
                let _ = req.respond(error_response(
                    tiny_http::StatusCode(500),
                    "internal_error",
                    e.to_string(),
                    cors.as_deref(),
                ));
            }
        }
    }

    Ok(())
}
