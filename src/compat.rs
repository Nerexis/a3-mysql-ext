pub mod error_code {
    pub const ALREADY_CONNECTED_TO_DATABASE: &str = "Already Connected to Database";
    pub const DATABASE_CONFIG: &str = "Database Config Error";
    pub const DATABASE_CONNECTION: &str = "Database Connection Error";
    pub const TLS_CONFIG: &str = "Error TLS Config";
    pub const PROTOCOL_NAME_TAKEN: &str = "Error Protocol Name Already Taken";
    pub const UNKNOWN_PROTOCOL: &str = "Error Unknown Protocol";
    pub const FAILED_TO_LOAD_PROTOCOL: &str = "Failed to Load Protocol";
    pub const INVALID_FORMAT: &str = "Error Invalid Format";
    pub const ERROR: &str = "ERROR";
    pub const INVALID_MESSAGE: &str = "Error Invalid Message";
    pub const LOGGER: &str = "Error LOGGER";
    pub const MARIADB_QUERY: &str = "Error MariaDBQueryException Exception";
    pub const MARIADB_CONNECTOR: &str = "Error MariaDBConnectorException Exception";
    pub const NO_CUSTOM_CALL: &str = "Error No Custom Call Not Found";
    pub const STRIP_CHAR_FOUND: &str = "Error Strip Char Found";
    pub const MARIADB_STATEMENT_0: &str = "Error MariaDBStatementException0 Exception";
    pub const MARIADB_STATEMENT_1: &str = "Error MariaDBStatementException1 Exception";
    pub const EXTDB3: &str = "Error extDB3Exception Exception";
}

#[derive(Debug, PartialEq, Eq)]
pub struct Failure {
    pub code: &'static str,
    pub detail: String,
}

impl Failure {
    pub fn new(code: &'static str, detail: impl Into<String>) -> Self {
        Self {
            code,
            detail: detail.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::error_code;

    #[test]
    fn original_extdb3_error_texts_remain_exact() {
        let codes = [
            (
                error_code::ALREADY_CONNECTED_TO_DATABASE,
                "Already Connected to Database",
            ),
            (error_code::DATABASE_CONFIG, "Database Config Error"),
            (error_code::DATABASE_CONNECTION, "Database Connection Error"),
            (
                error_code::PROTOCOL_NAME_TAKEN,
                "Error Protocol Name Already Taken",
            ),
            (error_code::UNKNOWN_PROTOCOL, "Error Unknown Protocol"),
            (
                error_code::FAILED_TO_LOAD_PROTOCOL,
                "Failed to Load Protocol",
            ),
            (error_code::INVALID_FORMAT, "Error Invalid Format"),
            (error_code::ERROR, "ERROR"),
            (error_code::INVALID_MESSAGE, "Error Invalid Message"),
            (error_code::LOGGER, "Error LOGGER"),
            (
                error_code::MARIADB_QUERY,
                "Error MariaDBQueryException Exception",
            ),
            (
                error_code::MARIADB_CONNECTOR,
                "Error MariaDBConnectorException Exception",
            ),
            (error_code::NO_CUSTOM_CALL, "Error No Custom Call Not Found"),
            (error_code::STRIP_CHAR_FOUND, "Error Strip Char Found"),
            (
                error_code::MARIADB_STATEMENT_0,
                "Error MariaDBStatementException0 Exception",
            ),
            (
                error_code::MARIADB_STATEMENT_1,
                "Error MariaDBStatementException1 Exception",
            ),
            (error_code::EXTDB3, "Error extDB3Exception Exception"),
        ];

        for (actual, expected) in codes {
            assert_eq!(actual, expected);
        }
    }
}
