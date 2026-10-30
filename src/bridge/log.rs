//! The audit lines of the calls a bridge answers itself, with `engine_lost` or
//! `engine_unavailable`: the engine never saw them, or ended before it answered, so only
//! the bridge can log them. A call's tool is logged only when the client's profile lists
//! it, as only such calls reach an engine's log, and its arguments not at all, since the
//! bridge doesn't read them.

use std::collections::BTreeSet;

use rmcp::model::Tool;
use serde_json::json;

use super::envelope;
use crate::Env;
use crate::audit::{Audit, Call, Caller};
use crate::error::ToolError;
use crate::session::Label;

/// Where the bridge logs, for which niri, and the tools it may name.
#[derive(Debug)]
pub(crate) struct Log {
    audit: Audit,
    instance: Option<String>,
    tools: BTreeSet<String>,
}

/// A call the bridge may answer itself: its tool and start, and the client's name.
#[derive(Debug)]
pub(crate) struct Asked {
    call: Call<'static>,
    client: String,
}

impl Log {
    /// Logs to the audit log `env` names, naming the tools of `tools`.
    pub(crate) fn new(env: &Env, tools: &[Tool]) -> Self {
        Self {
            audit: Audit::new(env.state_dir.clone()),
            instance: env.instance(),
            tools: tools.iter().map(|tool| tool.name.to_string()).collect(),
        }
    }

    /// The call of `tool` by the client named `client`, started now, when `tool` is one
    /// the client's profile lists.
    pub(crate) fn asked(&self, tool: &str, client: Option<&str>) -> Option<Asked> {
        self.tools.contains(tool).then(|| Asked {
            call: Call::start(tool.to_owned()),
            client: client.unwrap_or_default().to_owned(),
        })
    }

    /// The call `line` makes, if it is a `tools/call` of a listed tool, by the client its
    /// `_meta` names, or else its `initialize` line, `initialize`.
    pub(crate) fn line(&self, line: &[u8], initialize: Option<&[u8]>) -> Option<Asked> {
        let (tool, named) = envelope::called(line)?;
        let client = named.or_else(|| initialize.and_then(envelope::client));
        self.asked(&tool, client.as_deref())
    }

    /// Logs `asked`, which this bridge answered with `error`.
    pub(crate) fn answered(&self, asked: &Asked, error: &ToolError) {
        let session = Label::new(&asked.client, std::process::id()).to_string();
        let caller = Caller {
            session: &session,
            instance: self.instance.as_deref(),
        };
        let result = Ok(error.clone().into_result());
        self.audit.finish(&asked.call, caller, json!({}), &result);
    }
}
