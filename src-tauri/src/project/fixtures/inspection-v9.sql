-- 项目记录库 9 版（本机记录库之前每个项目一份）的表结构，供合并测试造旧库。
CREATE TABLE IF NOT EXISTS cycle_ids (
    id TEXT PRIMARY KEY NOT NULL DEFAULT (lower(hex(randomblob(16)))),
    created_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS workspace_instances (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    recipe_id TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS parts (
    id INTEGER PRIMARY KEY,
    ts INTEGER NOT NULL,
    sn TEXT NOT NULL,
    recipe_id TEXT,
    recipe_version INTEGER,
    recipe_revision TEXT,
    trigger_mode TEXT,
    verdict TEXT NOT NULL,
    plc_code INTEGER NOT NULL,
    fault_code INTEGER NOT NULL,
    reason TEXT NOT NULL,
    drain_ms INTEGER,
    frames_expected INTEGER NOT NULL,
    frames_received INTEGER NOT NULL,
    triggers INTEGER NOT NULL,
    retest_of INTEGER,
    software_version TEXT NOT NULL,
    judgement TEXT NOT NULL,
    frames TEXT NOT NULL,
    cycle_id TEXT,
    bundle_id TEXT,
    delivery_state TEXT NOT NULL,
    delivery_updated_at INTEGER NOT NULL,
    delivery_message TEXT,
    recording_state TEXT NOT NULL DEFAULT 'pending',
    recording_available INTEGER NOT NULL DEFAULT 0,
    recording_directory TEXT,
    recording_errors TEXT NOT NULL DEFAULT '[]',
    sample TEXT,
    source TEXT NOT NULL CHECK(source IN ('real','offlineSynthetic','offlineSample','offlineCandidate')),
    expected_total INTEGER,
    tolerance_total INTEGER,
    device_triggers INTEGER,
    camera_frames TEXT,
    flags TEXT,
    burst_judgement TEXT,
    mode TEXT NOT NULL DEFAULT 'production' CHECK(mode IN ('production','maintenanceCollect','maintenanceShadow'))
);
CREATE INDEX IF NOT EXISTS parts_ts ON parts(ts);
CREATE INDEX IF NOT EXISTS parts_source_ts ON parts(source, ts);
CREATE INDEX IF NOT EXISTS parts_sn ON parts(sn);
CREATE INDEX IF NOT EXISTS parts_recording_directory ON parts(recording_directory);
CREATE UNIQUE INDEX IF NOT EXISTS parts_cycle ON parts(cycle_id) WHERE cycle_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS parts_unresolved_delivery ON parts(cycle_id)
    WHERE cycle_id IS NOT NULL AND delivery_state IN ('pending','submitted','failed');
CREATE TABLE IF NOT EXISTS part_points (
    part_id INTEGER PRIMARY KEY REFERENCES parts(id) ON DELETE CASCADE,
    format INTEGER NOT NULL,
    data BLOB NOT NULL
);
CREATE TABLE IF NOT EXISTS recipe_snapshots (
    revision_id TEXT PRIMARY KEY,
    recipe_id TEXT NOT NULL,
    version INTEGER NOT NULL,
    json TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS part_shots (
    part_id INTEGER NOT NULL REFERENCES parts(id) ON DELETE CASCADE,
    k INTEGER NOT NULL,
    shot_id TEXT NOT NULL,
    camera TEXT NOT NULL,
    view INTEGER NOT NULL CHECK(view BETWEEN 1 AND 3),
    session TEXT,
    ordinal TEXT,
    frame_counter TEXT,
    trigger_counter TEXT,
    status TEXT NOT NULL,
    error TEXT,
    score REAL,
    ms INTEGER,
    raw_files TEXT NOT NULL,
    PRIMARY KEY (part_id, k),
    UNIQUE (part_id, shot_id)
);
CREATE TABLE IF NOT EXISTS part_frames (
    part_id INTEGER NOT NULL REFERENCES parts(id) ON DELETE CASCADE,
    camera INTEGER NOT NULL CHECK(camera >= 0),
    seq INTEGER NOT NULL CHECK(seq >= 1),
    status TEXT NOT NULL,
    frame_counter TEXT,
    trigger_counter TEXT,
    region INTEGER,
    metrics TEXT,
    raw_file TEXT,
    PRIMARY KEY (part_id, camera, seq)
);
PRAGMA user_version = 9;
