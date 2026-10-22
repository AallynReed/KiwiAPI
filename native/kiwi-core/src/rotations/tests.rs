use super::*;

const fn at(y: i64, mo: i64, d: i64, h: i64, mi: i64, s: i64, us: i64) -> i64 {
    utc(y, mo, d, h) + mi * MINUTE + s * US + us
}

#[test]
fn anchors() {
    assert_eq!(FIRST_WEEK_BUFF / US, 1_584_921_600);
    assert_eq!(D15_EPOCH / US, 1_718_708_400);
    assert_eq!(LUXION_EPOCH, 1_760_094_000);
    assert_eq!(DELVE_WEEK_ONE / US, 1_762_167_600);
    assert_eq!(weekday(utc(1970, 1, 1, 0)), 3);
    assert_eq!(weekday(utc(2026, 10, 22, 0)), 3);
    assert_eq!(weekday(utc(1969, 12, 29, 0)), 0);
}

#[test]
fn server_time_resets() {
    let st = server_time(at(2026, 10, 22, 12, 34, 56, 123_456));
    assert_eq!(st.now_unix, 1_792_672_496);
    assert_eq!(WEEKDAYS[st.trove_weekday], "Thursday");
    assert_eq!(st.daily_reset_at, 1_792_753_200);
    assert_eq!(st.weekly_reset_at, 1_793_012_400);
    let before = server_time(at(2026, 10, 22, 10, 59, 59, 999_999));
    assert_eq!(before.daily_reset_at, utc(2026, 10, 22, 11) / US);
    assert_eq!(WEEKDAYS[before.trove_weekday], "Wednesday");
}

#[test]
fn challenge_friday_half_hours() {
    let w = challenge_window(at(2026, 10, 23, 11, 35, 1, 5));
    assert!(w.is_friday_window && w.active);
    assert_eq!(w.starts_at, at(2026, 10, 23, 11, 30, 0, 0) / US);
    assert_eq!(w.seconds_remaining, 899);
    let thu = challenge_window(at(2026, 10, 23, 10, 59, 0, 0));
    assert!(!thu.is_friday_window && !thu.active);
    assert_eq!(thu.starts_at, at(2026, 10, 23, 10, 0, 0, 0) / US);
}

#[test]
fn classify_literals() {
    assert_eq!(classify_challenge(Some(" RAMPAGE ALERT!\u{1f}")), Some("rampage"));
    assert_eq!(classify_challenge(Some("Rampage")), Some("dungeon"));
    assert_eq!(classify_challenge(Some(" \t")), None);
    assert_eq!(classify_challenge(None), None);
}

#[test]
fn luxion_grid() {
    assert_eq!(next_grid_slot(LUXION_EPOCH), LUXION_EPOCH);
    assert_eq!(next_grid_slot(LUXION_EPOCH + 1), LUXION_EPOCH + 27 * 3600);
    assert_eq!(next_grid_slot(LUXION_EPOCH - 1), LUXION_EPOCH);
    let sched = schedule_for(1_785_754_800, Some(1_785_770_525), None);
    assert_eq!(sched.len(), 6);
    assert_eq!(sched[0].state.as_deref(), Some("Day 1"));
    assert!(sched.iter().all(|w| (w.starts_at - LUXION_EPOCH) % (27 * 3600) == 0));
    let away = luxion(1_785_754_800, Some(1_785_770_525), None, 1_786_359_600 * US);
    assert!(!away.active && away.seconds_remaining == 0 && away.next_window.is_none());
}

#[test]
fn delve_weeks() {
    assert_eq!(delve_week_id(DELVE_WEEK_ONE), 1);
    assert_eq!(delve_week_id(DELVE_WEEK_ONE - 1), 0);
    assert_eq!(delve_week_id(DELVE_WEEK_ONE + WEEK), 2);
    assert_eq!(delve_next_pull(at(2026, 10, 19, 12, 30, 0, 0)), 30 * MINUTE);
    assert_eq!(delve_next_pull(at(2026, 10, 20, 10, 59, 59, 999_999)), US);
    assert_eq!(delve_next_pull(at(2026, 10, 21, 11, 0, 0, 0)), DAY);
}

#[test]
fn rotations_shapes() {
    let now = at(2026, 10, 22, 12, 34, 56, 123_456);
    assert_eq!(biome_rotation(now, 16).len(), 17);
    assert_eq!(wild_mana(now, 8).len(), 8);
    let st = stampy(now, 8);
    assert_eq!(st.len(), 8);
    assert!(st[0].ends_at * US > now);
    let g = gardening(now);
    assert!(g.upcoming.len() <= 10 && g.upcoming.windows(2).all(|w| w[0].starts_at <= w[1].starts_at));
    let cal = yearly_calendar(now, None, &[]);
    assert!(cal.events.windows(2).all(|w| (w[0].starts_at, w[0].kind) <= (w[1].starts_at, w[1].kind)));
    assert!(cal.events.iter().all(|e| e.kind != "weekly_buff"));
}
