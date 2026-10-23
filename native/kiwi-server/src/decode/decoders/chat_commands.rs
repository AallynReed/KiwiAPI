//! `chat_commands.py`: the chat commands the in-game help lists (`prefabs/help`).

use crate::{
    decode::{
        json::J,
        registry::{Built, Ctx},
        value::{W, parse},
    },
    obj,
};

pub fn build(ctx: &Ctx) -> Built {
    let data = ctx.tree.read("prefabs/help.binfab").unwrap_or_default();
    let pf = parse(&data).map_err(|_| crate::decode::registry::DecodeError {
        kind: "WireError",
        msg: "no reading fills the object exactly".into(),
    })?;
    let mut rows = Vec::new();
    for row in pf.root_leaf().iter_of(0) {
        let r = row.leaf();
        let Some(cmd) = r.str(0).filter(|s| !s.is_empty()) else { continue };
        let args: Vec<J> = r.iter_of(2).iter().filter_map(W::as_str).filter(|a| !a.is_empty()).map(J::from).collect();
        rows.push(obj! {"command" => cmd, "description" => r.str(1).unwrap_or(""), "args" => args});
    }
    Ok(obj! {"commands" => rows})
}
