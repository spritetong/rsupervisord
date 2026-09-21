// Copyright (c) 2026 Sprite Tong <spritetong@gmail.com>
//
// Licensed under the MIT License.
// SPDX-License-Identifier: MIT

pub mod fault;
pub mod supervisor;
pub mod system;
pub mod types;
pub mod wire;

use crate::compat::xmlrpc::fault::Fault;
use crate::compat::xmlrpc::supervisor::{SupervisorRpcContext, handle_supervisor_method};
use crate::compat::xmlrpc::wire::{MethodCall, Value, serialize_response};
use crate::server::api::AppState;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::header::{CONTENT_TYPE, WWW_AUTHENTICATE};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};

/// Axum route handler for POST `/RPC2`.
pub async fn xmlrpc_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    // 1. Verify HTTP Basic Authentication if configured
    if let Some(ref basic) = state.basic_auth {
        let authorized = headers
            .get("Authorization")
            .and_then(|v| v.to_str().ok())
            .and_then(crate::server::auth::extract_basic_auth)
            .map(|(u, p)| basic.verify(&u, &p))
            .unwrap_or(false);

        if !authorized {
            return (
                StatusCode::UNAUTHORIZED,
                [(WWW_AUTHENTICATE, "Basic realm=\"default\"")],
                "Unauthorized\n",
            )
                .into_response();
        }
    }

    // 2. Decode raw XML request body
    let xml_str = match std::str::from_utf8(&body) {
        Ok(s) => s,
        Err(_) => {
            let res = Err(Fault::incorrect_params("Request body is not valid UTF-8"));
            return xml_response(res);
        }
    };

    if xml_str.trim().is_empty() {
        let res = Err(Fault::incorrect_params("Empty request body"));
        return xml_response(res);
    }

    // 3. Parse XML-RPC MethodCall
    let call = match MethodCall::parse(xml_str) {
        Ok(c) => c,
        Err(fault) => return xml_response(Err(fault)),
    };

    // 4. Dispatch method
    let ctx = SupervisorRpcContext {
        manager: state.manager.clone(),
        config_path: state.config_path.clone(),
    };

    let result = dispatch_call(&ctx, &call.name, &call.params).await;
    xml_response(result)
}

/// Dispatches a single XML-RPC method call to the appropriate subsystem.
pub fn dispatch_call<'a>(
    ctx: &'a SupervisorRpcContext,
    method: &'a str,
    params: &'a [Value],
) -> system::BoxFuture<'a, Result<Value, Fault>> {
    Box::pin(async move {
        if method.starts_with("system.") {
            match method {
                "system.listMethods" => Ok(system::list_methods()),
                "system.methodHelp" => {
                    let name = params.first().and_then(|v| v.as_str()).ok_or_else(|| {
                        Fault::incorrect_params("system.methodHelp requires method name string")
                    })?;
                    system::method_help(name)
                }
                "system.methodSignature" => {
                    let name = params.first().and_then(|v| v.as_str()).ok_or_else(|| {
                        Fault::incorrect_params(
                            "system.methodSignature requires method name string",
                        )
                    })?;
                    system::method_signature(name)
                }
                "system.multicall" => {
                    system::handle_multicall(params, move |sub_method, sub_params| {
                        dispatch_call(ctx, sub_method, sub_params)
                    })
                    .await
                }
                _ => Err(Fault::unknown_method(method)),
            }
        } else if method.starts_with("supervisor.") {
            handle_supervisor_method(ctx, method, params).await
        } else {
            Err(Fault::unknown_method(method))
        }
    })
}

fn xml_response(res: Result<Value, Fault>) -> Response {
    let xml = serialize_response(res);
    (
        StatusCode::OK,
        [(CONTENT_TYPE, "text/xml; charset=utf-8")],
        xml,
    )
        .into_response()
}
