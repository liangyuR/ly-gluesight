use super::*;

#[test]
fn offline_validated_release_runs_only_in_offline_mode() {
    let cases = [
        ("真机 + 离线验证版本 → 拒绝", ValidationSite::Offline, false, false),
        ("离线 + 离线验证版本 → 可运行", ValidationSite::Offline, true, true),
        ("真机 + 真机验证版本 → 可运行", ValidationSite::Real, false, true),
    ];
    for (name, site, offline, admitted) in cases {
        assert_eq!(site_admits(site, offline).is_ok(), admitted, "{name}");
    }
}
