//! `app/trove/mod_categories.py`: the mod category vocabulary and its bitmask.
//! Bit order is a public contract (append-only).

pub const NAMES: [&str; 19] = [
    "Allies",
    "Banners",
    "Boats and Sails",
    "Cosmetics",
    "Costumes",
    "Dragons",
    "Fishing",
    "GUI",
    "Helmets",
    "Language",
    "Mag Riders",
    "Mounts",
    "NPCs",
    "Wings",
    "Automation",
    "Optimization",
    "Reskin",
    "Waypoint",
    "Radar",
];

pub const ALL_MASK: u64 = (1 << NAMES.len()) - 1;

/// `flags_from_tags`: each tag stripped (Python whitespace) and lowercased.
pub fn flags_from_tags<'a>(tags: impl IntoIterator<Item = &'a str>) -> u64 {
    tags.into_iter().fold(0, |flags, tag| {
        let tag = crate::pystr::strip(tag).to_lowercase();
        flags | NAMES.iter().position(|n| n.to_lowercase() == tag).map_or(0, |i| 1 << i)
    })
}

/// `tags_from_flags`.
pub fn tags_from_flags(flags: u64) -> Vec<&'static str> {
    let mask = flags & ALL_MASK;
    NAMES.iter().enumerate().filter(|(i, _)| mask & (1 << i) != 0).map(|(_, n)| *n).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        assert_eq!(flags_from_tags(" dragons,FISHING ,nope".split(',')), 96);
        assert_eq!(tags_from_flags(96), ["Dragons", "Fishing"]);
        assert_eq!(flags_from_tags(["Radar"]), 1 << 18);
    }
}
