// SPDX-License-Identifier: MPL-2.0

//! `reference/EVENTS.md`, generated from the registry — the
//! koru-env/`koru-keys` mechanism: never hand-edited, asserted current by a
//! test, rewritten with `KORU_UPDATE_EVENTS=1 cargo test -p koru-events`.
//!
//! **It sits in `reference/`, beside the other generated references,** rather
//! than in `docs/`, because the repository builds and tests without the
//! `docs/` checkout and this file is asserted by a test. It is still only
//! readable next to what an event *is*: the table says a name, a class and a
//! policy, and every one of those words is defined in
//! `docs/architecture/eventbus/README.md`, which the links below reach.

use crate::registry::{AttrSpec, REGISTRY};

pub fn markdown() -> String {
    let mut out = String::new();
    out.push_str("# Events\n\n");
    out.push_str("<!-- Generated from crates/koru-events/src/registry.rs. Do not edit by hand;\n");
    out.push_str(
        "     run `KORU_UPDATE_EVENTS=1 cargo test -p koru-events` to regenerate. -->\n\n",
    );
    out.push_str(
        "**Every event this system publishes, and what each one is for.** One \
         registry,\nin `crates/koru-events/src/registry.rs`, which the code \
         compiles against — so a\nname that is not a row here cannot be sent, \
         and this table cannot drift from\nwhat is emitted.\n\n\
         Read the columns as: what happened (**Event**), which payload shape \
         (**v** —\nbumped only on a breaking change, and a `(name, version)` \
         pair is never reused\nwith a different meaning), what kind of fact it \
         is (**Class**), whether it may\nleave the machine (**Telemetry**), \
         and the typed values it carries\n(**Attributes**).\n\n\
         **Class** is `domain` — something happened to content, kept 90 days — \
         or\n`instrumentation` — an observation about the app, kept 14. \
         **Telemetry** is\n`operational` (exportable service health), \
         `anonymous-usage` or `pseudonymous-usage`\n(exportable product \
         measurement), `local-reference` (never leaves), or `never`.\n\n\
         **Attributes are typed and there is no string.** A value is an `id`, \
         a `time`,\na `token` from a closed vocabulary the row declares, a \
         `count`, an `int`, a\n`version`, a `bucket` or a `flag` — so content \
         cannot be smuggled through one.\nBefore anything is exported, every \
         `id` and every `local-reference` attr, and\nall actor, device, \
         profile, workspace and subject identity, are stripped\n\
         mechanically — whatever the telemetry policy says.\n\n\
         **To add or change an event**, add the row and regenerate; never write \
         a bare\nname at a call site. An attribute an exportable policy would \
         ship needs a\nprivacy-class review first.\n\n\
         What an event *is*, and where it goes: \
         [README.md](../docs/architecture/eventbus/README.md).\nWhat is still \
         owed: [AGENTIC_FLOWS.md](../docs/architecture/eventbus/AGENTIC_FLOWS.md).\n\n",
    );
    out.push_str("| Event | v | Class | Telemetry | Attributes | Notes |\n");
    out.push_str("| --- | --- | --- | --- | --- | --- |\n");
    for spec in REGISTRY {
        let attrs = if spec.attrs.is_empty() {
            "—".to_string()
        } else {
            spec.attrs
                .iter()
                .map(attr_cell)
                .collect::<Vec<_>>()
                .join("<br>")
        };
        out.push_str(&format!(
            "| `{}` | {} | {} | {} | {} | {} |\n",
            spec.name,
            spec.version,
            spec.class.as_str(),
            spec.telemetry.as_str(),
            attrs,
            collapse(spec.doc),
        ));
    }
    out
}

fn attr_cell(attr: &AttrSpec) -> String {
    let requirement = if attr.required { "" } else { "?" };
    let vocabulary = if attr.vocabulary.is_empty() {
        String::new()
    } else {
        format!(" ({})", attr.vocabulary.join(" \\| "))
    };
    format!(
        "`{}{}` : {}/{}{}",
        attr.key,
        requirement,
        attr.kind.as_str(),
        attr.class.as_str(),
        vocabulary
    )
}

/// Doc strings are wrapped in the source; a table cell wants one line.
fn collapse(doc: &str) -> String {
    doc.split_whitespace().collect::<Vec<_>>().join(" ")
}
