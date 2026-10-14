ALTER USER 'extdb_test'@'%' IDENTIFIED WITH caching_sha2_password BY 'extdb_test_only' REQUIRE SSL;
GRANT ALL ON extdb_test.* TO 'extdb_test'@'%';
CREATE USER 'extdb_reference'@'%' IDENTIFIED WITH caching_sha2_password BY 'extdb_test_only';
GRANT ALL ON extdb_test.* TO 'extdb_reference'@'%';
CREATE TABLE extdb_test.probe_insert (id BIGINT AUTO_INCREMENT PRIMARY KEY, name VARCHAR(255) NOT NULL);
