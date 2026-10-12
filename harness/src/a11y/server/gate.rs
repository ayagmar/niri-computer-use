//! How the element actions' last gate went in a run, from the server's audit log: how many
//! calls reached it, how many ran a second pass or were refused with `element_busy`, how
//! long each pass took, and how old its answers were when the call went out, against the
//! gate's 250 ms bound.

use serde_json::Value;

use crate::failure::{Context as _, Failure, Result};
use crate::mcp::field;

/// The server's bound on the last gate, in milliseconds.
const BOUND_MS: f64 = 250.0;

/// What the audit log says of the last gate.
#[derive(Debug, Default, PartialEq)]
pub(super) struct Gate {
    /// Element actions that ran at least one pass.
    pub(super) calls: usize,
    /// Of those, the ones that ran a second pass.
    pub(super) retried: usize,
    /// Of those, the ones refused with `element_busy`.
    pub(super) busy: usize,
    /// Every pass, in milliseconds, sorted.
    pub(super) passes_ms: Vec<f64>,
    /// How old the answers were when each call went out, in milliseconds, sorted.
    pub(super) sent_ms: Vec<f64>,
}

impl Gate {
    /// Reads the `gate` of every element action in `audit`, one JSON object per line.
    pub(super) fn of(audit: &str) -> Result<Self> {
        let mut gate = Self::default();
        for line in audit.lines() {
            let entry: Value = serde_json::from_str(line).context("parse an audit line")?;
            let tool = field(&entry, "/tool");
            if tool != "activate_element" && tool != "set_element_text" {
                continue;
            }
            let Some(passes) = field(&entry, "/args/gate/passes_ms").as_array() else {
                continue;
            };
            gate.calls += 1;
            gate.retried += usize::from(passes.len() > 1);
            gate.busy += usize::from(field(&entry, "/error") == "element_busy");
            gate.passes_ms
                .extend(passes.iter().filter_map(Value::as_f64));
            gate.sent_ms
                .extend(field(&entry, "/args/gate/sent_ms").as_f64());
        }
        gate.passes_ms.sort_by(f64::total_cmp);
        gate.sent_ms.sort_by(f64::total_cmp);
        if gate.calls == 0 {
            return Err(Failure::new(
                "M9b: no element action in the audit log has the last gate's times",
            ));
        }
        Ok(gate)
    }

    /// One line for the run's record.
    pub(super) fn line(&self) -> String {
        let longest = self.passes_ms.last().copied().unwrap_or_default();
        format!(
            "M9b: last gate: {} element actions ran it, {} ran a second pass, {} element_busy; pass ms {}; age when sent ms {}; the longest pass used {:.1}% of {BOUND_MS} ms",
            self.calls,
            self.retried,
            self.busy,
            spread(&self.passes_ms),
            spread(&self.sent_ms),
            longest / BOUND_MS * 100.0,
        )
    }
}

/// The minimum, median, 95th percentile and maximum of `sorted`.
fn spread(sorted: &[f64]) -> String {
    let at = |share: usize| {
        let index = (sorted.len().saturating_sub(1) * share).div_ceil(100);
        sorted.get(index).copied().unwrap_or_default()
    };
    format!(
        "min {} median {} p95 {} max {} (n {})",
        at(0),
        at(50),
        at(95),
        at(100),
        sorted.len()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_second_passes_and_busy_refusals_of_element_actions_only() {
        let audit = [
            r#"{"tool":"activate_element","args":{"gate":{"passes_ms":[9.5],"sent_ms":10.0}},"error":null}"#,
            r#"{"tool":"set_element_text","args":{"gate":{"passes_ms":[250.0,12.0],"sent_ms":12.5}},"error":null}"#,
            r#"{"tool":"activate_element","args":{"gate":{"passes_ms":[250.0,250.0]}},"error":"element_busy"}"#,
            r#"{"tool":"activate_element","args":{},"error":"element_stale"}"#,
            r#"{"tool":"click","args":{"gate":{"passes_ms":[1.0]}},"error":null}"#,
        ]
        .join("\n");
        let gate = Gate::of(&audit).unwrap();
        assert_eq!((gate.calls, gate.retried, gate.busy), (3, 2, 1));
        assert_eq!(gate.passes_ms, [9.5, 12.0, 250.0, 250.0, 250.0]);
        assert_eq!(gate.sent_ms, [10.0, 12.5]);
        assert!(Gate::of(r#"{"tool":"activate_element","args":{}}"#).is_err());
    }
}
