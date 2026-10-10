CREATE TABLE IF NOT EXISTS extdb_test.account (
    uid VARCHAR(64) NOT NULL PRIMARY KEY,
    clan_id INT NULL,
    name VARCHAR(255) NULL,
    next_raid_at DATETIME NULL
);

CREATE TABLE IF NOT EXISTS extdb_test.vehicle (
    id BIGINT NOT NULL PRIMARY KEY,
    class VARCHAR(100) NOT NULL,
    spawned_at DATETIME NOT NULL,
    account_uid VARCHAR(64) NOT NULL,
    is_locked TINYINT(1) NOT NULL,
    fuel DOUBLE NOT NULL,
    damage DOUBLE NOT NULL,
    hitpoints LONGTEXT NULL,
    position_x DOUBLE NOT NULL,
    position_y DOUBLE NOT NULL,
    position_z DOUBLE NOT NULL,
    direction_x DOUBLE NOT NULL,
    direction_y DOUBLE NOT NULL,
    direction_z DOUBLE NOT NULL,
    up_x DOUBLE NOT NULL,
    up_y DOUBLE NOT NULL,
    up_z DOUBLE NOT NULL,
    cargo_items LONGTEXT NULL,
    cargo_magazines LONGTEXT NULL,
    cargo_weapons LONGTEXT NULL,
    pin_code VARCHAR(16) NULL,
    vehicle_texture VARCHAR(255) NULL,
    deleted_at DATETIME NULL,
    destroyed_at DATETIME NULL,
    territory_id BIGINT NULL,
    money BIGINT NOT NULL,
    custom_name VARCHAR(255) NULL,
    is_engine_on TINYINT(1) NOT NULL,
    is_radar_on TINYINT(1) NOT NULL
);

INSERT INTO extdb_test.account (uid, clan_id, name, next_raid_at)
VALUES ('fixture-player', NULL, 'Fixture Owner', '2099-01-01 00:00:00')
ON DUPLICATE KEY UPDATE
    clan_id = VALUES(clan_id),
    name = VALUES(name),
    next_raid_at = VALUES(next_raid_at);

INSERT INTO extdb_test.vehicle (
    id, class, spawned_at, account_uid, is_locked, fuel, damage, hitpoints,
    position_x, position_y, position_z, direction_x, direction_y, direction_z,
    up_x, up_y, up_z, cargo_items, cargo_magazines, cargo_weapons, pin_code,
    vehicle_texture, deleted_at, destroyed_at, territory_id, money, custom_name,
    is_engine_on, is_radar_on
)
VALUES (
    1, 'C_Offroad_01_F', '2026-10-10 12:00:00', 'fixture-player', 1, 0.75, 0.1, '[]',
    1, 2, 3, 0, 1, 0, 0, 0, 1, '[]', '[]', '[]', '1234', '', NULL, NULL, NULL,
    50, 'TestRig', 1, 0
)
ON DUPLICATE KEY UPDATE class = VALUES(class), spawned_at = VALUES(spawned_at);

CREATE TABLE IF NOT EXISTS extdb_test.player (
    id BIGINT NOT NULL PRIMARY KEY,
    name VARCHAR(255) NULL,
    damage DOUBLE NULL,
    hunger DOUBLE NULL,
    thirst DOUBLE NULL,
    alcohol DOUBLE NULL,
    oxygen_remaining DOUBLE NULL,
    bleeding_remaining DOUBLE NULL,
    hitpoints LONGTEXT NULL,
    direction DOUBLE NULL,
    position_x DOUBLE NULL,
    position_y DOUBLE NULL,
    position_z DOUBLE NULL,
    assigned_items LONGTEXT NULL,
    backpack VARCHAR(255) NULL,
    backpack_items LONGTEXT NULL,
    backpack_magazines LONGTEXT NULL,
    backpack_weapons LONGTEXT NULL,
    current_weapon VARCHAR(255) NULL,
    goggles VARCHAR(255) NULL,
    handgun_items LONGTEXT NULL,
    handgun_weapon VARCHAR(255) NULL,
    headgear VARCHAR(255) NULL,
    binocular VARCHAR(255) NULL,
    loaded_magazines LONGTEXT NULL,
    primary_weapon VARCHAR(255) NULL,
    primary_weapon_items LONGTEXT NULL,
    secondary_weapon VARCHAR(255) NULL,
    secondary_weapon_items LONGTEXT NULL,
    uniform VARCHAR(255) NULL,
    uniform_items LONGTEXT NULL,
    uniform_magazines LONGTEXT NULL,
    uniform_weapons LONGTEXT NULL,
    vest VARCHAR(255) NULL,
    vest_items LONGTEXT NULL,
    vest_magazines LONGTEXT NULL,
    vest_weapons LONGTEXT NULL,
    temperature DOUBLE NULL,
    wetness DOUBLE NULL,
    money BIGINT NULL,
    ace_medical_status LONGTEXT NULL,
    ace_dagr_waypoints LONGTEXT NULL
);

INSERT INTO extdb_test.player (id, name)
VALUES (1, 'Old Player')
ON DUPLICATE KEY UPDATE name = VALUES(name);

CREATE TABLE IF NOT EXISTS extdb_test.units (
    id BIGINT NOT NULL PRIMARY KEY,
    owner_uid VARCHAR(64) NOT NULL,
    clan_id BIGINT NULL,
    classname VARCHAR(100) NOT NULL,
    name VARCHAR(255) NULL,
    loadout LONGTEXT NULL,
    position_x DOUBLE NOT NULL,
    position_y DOUBLE NOT NULL,
    position_z DOUBLE NOT NULL,
    combat_mode VARCHAR(32) NULL,
    combat_behaviour VARCHAR(32) NULL,
    money BIGINT NULL,
    vehicle_id BIGINT NULL,
    assigned_veh_role LONGTEXT NULL,
    group_id BIGINT NULL,
    damage DOUBLE NULL,
    deleted_at DATETIME NULL
);

CREATE TABLE IF NOT EXISTS extdb_test.construction_unit_storage (
    unit_id BIGINT NOT NULL PRIMARY KEY,
    construction_id BIGINT NULL
);

CREATE TABLE IF NOT EXISTS extdb_test.construction (
    id BIGINT NOT NULL PRIMARY KEY,
    class VARCHAR(100) NULL,
    territory_id BIGINT NULL,
    deleted_at DATETIME NULL,
    damage DOUBLE NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS extdb_test.container (
    id BIGINT NOT NULL PRIMARY KEY,
    territory_id BIGINT NULL,
    deleted_at DATETIME NULL
);

CREATE TABLE IF NOT EXISTS extdb_test.clan (
    id BIGINT NOT NULL PRIMARY KEY,
    leader_uid VARCHAR(64) NULL,
    territory_color INT NULL,
    territory_brush INT NULL,
    name VARCHAR(255) NULL
);

CREATE TABLE IF NOT EXISTS extdb_test.color (
    id_color INT NOT NULL PRIMARY KEY,
    color_name VARCHAR(100) NULL
);

CREATE TABLE IF NOT EXISTS extdb_test.brush (
    id_brush INT NOT NULL PRIMARY KEY,
    brush_name VARCHAR(100) NULL
);

CREATE TABLE IF NOT EXISTS extdb_test.territory (
    id BIGINT NOT NULL PRIMARY KEY,
    owner_uid VARCHAR(64) NOT NULL,
    name VARCHAR(255) NULL,
    position_x DOUBLE NOT NULL,
    position_y DOUBLE NOT NULL,
    position_z DOUBLE NOT NULL,
    radius DOUBLE NOT NULL,
    level INT NOT NULL,
    flag_texture VARCHAR(255) NULL,
    flag_stolen TINYINT(1) NOT NULL,
    flag_stolen_by_uid VARCHAR(64) NULL,
    last_paid_at DATETIME NULL,
    build_rights LONGTEXT NULL,
    moderators LONGTEXT NULL,
    deleted_at DATETIME NULL,
    created_at DATETIME NOT NULL,
    last_ownership_change DATETIME NOT NULL,
    is_strategic TINYINT(1) NOT NULL
);

INSERT INTO extdb_test.units (
    id, owner_uid, clan_id, classname, name, loadout,
    position_x, position_y, position_z, combat_mode, combat_behaviour,
    money, vehicle_id, assigned_veh_role, group_id, damage, deleted_at
)
VALUES (
    42, 'fixture-player', NULL, 'O_Soldier_F', 'Fixture Unit', '[]',
    1, 2, 3, 'BLUE', 'AWARE', 25, NULL, '[]', NULL, 0.25, NULL
)
ON DUPLICATE KEY UPDATE name = VALUES(name);

INSERT INTO extdb_test.territory (
    id, owner_uid, name, position_x, position_y, position_z, radius, level,
    flag_texture, flag_stolen, flag_stolen_by_uid, last_paid_at, build_rights,
    moderators, deleted_at, created_at, last_ownership_change, is_strategic
)
VALUES (
    7, 'fixture-player', 'Fixture Territory', 4, 5, 6, 50, 1,
    'flag.jpg', 0, NULL, '2026-10-09 12:00:00', '[]', '[]', NULL,
    CURDATE(), CURDATE(), 0
)
ON DUPLICATE KEY UPDATE
    name = VALUES(name),
    created_at = VALUES(created_at),
    last_ownership_change = VALUES(last_ownership_change);
