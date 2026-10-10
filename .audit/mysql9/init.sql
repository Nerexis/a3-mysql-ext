ALTER USER 'extdb_test'@'%' IDENTIFIED WITH caching_sha2_password BY 'extdb_test_only' REQUIRE SSL;
GRANT ALL ON extdb_test.* TO 'extdb_test'@'%';
CREATE USER 'extdb_reference'@'%' IDENTIFIED WITH caching_sha2_password BY 'extdb_test_only';
GRANT ALL ON extdb_test.* TO 'extdb_reference'@'%';
CREATE TABLE extdb_test.probe_insert (id BIGINT AUTO_INCREMENT PRIMARY KEY, name VARCHAR(255) NOT NULL);

CREATE TABLE IF NOT EXISTS extdb_test.account (
    uid VARCHAR(64) NOT NULL PRIMARY KEY,
    clan_id INT NULL
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

INSERT INTO extdb_test.account (uid, clan_id)
VALUES ('fixture-player', NULL)
ON DUPLICATE KEY UPDATE clan_id = VALUES(clan_id);

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
