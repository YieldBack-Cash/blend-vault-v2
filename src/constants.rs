/// 1 with 7 decimal places
pub const SCALAR_7: i128 = 1_0000000;
/// 1 with 12 decimal places
pub const SCALAR_12: i128 = 1_000_000_000_000;
// seconds per year
pub const SECONDS_PER_YEAR: i128 = 31536000;

/// BLND token contract address (mainnet)
#[cfg(not(test))]
pub const BLND_TOKEN: &str = "CD25MNVTZDL4Y3XBCPCJXGXATV5WUHHOWMYFF4YBEGU5FCPGMYTVG5JY";

/// BLND token contract address (testnet)
#[cfg(test)]
pub const BLND_TOKEN: &str = "CB22KRA3YZVCNCQI64JQ5WE7UY2VAV7WFLK6A2JN3HEX56T2EDAFO7QF";
