use crate::{
    constants::SCALAR_12,
    errors::BlendVaultError,
    pool,
    storage,
    validator::require_positive,
};
use soroban_fixed_point_math::{i128, FixedPoint};
use soroban_sdk::{contracttype, panic_with_error, unwrap::UnwrapOptimized, Address, Env};

#[derive(Clone)]
#[cfg_attr(test, derive(Debug))]
#[contracttype]
pub struct VaultData {
    /// The timestamp of the last update
    pub last_update_timestamp: u64,
    /// The reserve's last bRate
    pub b_rate: i128,
    /// The total shares issued by the reserve vault
    pub total_shares: i128,
    /// The total bToken deposits owned by the reserve vault depositors.
    pub total_b_tokens: i128,
}

impl VaultData {
    /// Converts a b_token amount to shares rounding down
    pub fn b_tokens_to_shares_down(&self, amount: i128) -> i128 {
        if self.total_shares == 0 || self.total_b_tokens == 0 {
            return amount;
        }
        amount
            .fixed_mul_floor(self.total_shares, self.total_b_tokens)
            .unwrap_optimized()
    }

    /// Converts a b_token amount to shares rounding up
    pub fn b_tokens_to_shares_up(&self, amount: i128) -> i128 {
        if self.total_shares == 0 || self.total_b_tokens == 0 {
            return amount;
        }
        amount
            .fixed_mul_ceil(self.total_shares, self.total_b_tokens)
            .unwrap_optimized()
    }

    /// Converts a share amount to a b_token amount rounding down
    pub fn shares_to_b_tokens_down(&self, amount: i128) -> i128 {
        if self.total_shares == 0 {
            // an empty vault mints shares 1:1 with bTokens, so quote the same
            // rate a first deposit would get
            return amount;
        }
        amount
            .fixed_div_floor(self.total_shares, self.total_b_tokens)
            .unwrap_optimized()
    }

    /// Converts a b_token amount to an underlying token amount rounding down
    pub fn b_tokens_to_underlying_down(&self, amount: i128) -> i128 {
        amount
            .fixed_mul_floor(self.b_rate, SCALAR_12)
            .unwrap_optimized()
    }

    /// Converts an underlying amount to a b_token amount rounding down
    pub fn underlying_to_b_tokens_down(&self, amount: i128) -> i128 {
        amount
            .fixed_div_floor(self.b_rate, SCALAR_12)
            .unwrap_optimized()
    }

    /// Converts an underlying amount to a b_token amount rounding up
    pub fn underlying_to_b_tokens_up(&self, amount: i128) -> i128 {
        amount
            .fixed_div_ceil(self.b_rate, SCALAR_12)
            .unwrap_optimized()
    }

    /// Updates the reserve's bRate
    fn update_rate(&mut self, e: &Env, pool: &Address, asset: &Address) {
        self.last_update_timestamp = e.ledger().timestamp();
        self.b_rate = pool::reserve_b_rate(e, pool, asset);
    }
}

/// Get the reserve vault from storage and update the bRate
///
/// ### Arguments
/// * `pool` - The pool address
/// * `asset` - The asset address
///
/// ### Returns
/// * `VaultData` - The updated reserve vault
pub fn get_vault_updated(e: &Env, pool: &Address, asset: &Address) -> VaultData {
    let mut vault = storage::get_vault_data(e);
    vault.update_rate(e, pool, asset);
    vault
}

/// Deposit into the vault. Does not perform the call to the pool to deposit the tokens.
///
/// ### Returns
/// * `(i128, i128)` - (The amount of b_tokens minted to the vault, the amount of shares minted to the user)
///
/// ### Panics
/// * If the underlying amount is less than or equal to 0
pub fn deposit(
    e: &Env,
    pool: &Address,
    asset: &Address,
    user: &Address,
    amount: i128,
) -> (i128, i128) {
    let mut vault = get_vault_updated(e, pool, asset);
    let mut user_shares = storage::get_vault_shares(e, user);

    let b_tokens_amount = vault.underlying_to_b_tokens_down(amount);
    require_positive(e, b_tokens_amount, BlendVaultError::InvalidBTokensMinted);
    let share_amount = vault.b_tokens_to_shares_down(b_tokens_amount);
    require_positive(e, share_amount, BlendVaultError::InvalidSharesMinted);

    vault.total_shares += share_amount;
    vault.total_b_tokens += b_tokens_amount;
    user_shares += share_amount;
    storage::set_vault_data(e, &vault);
    storage::set_vault_shares(e, user, user_shares);
    (b_tokens_amount, share_amount)
}

/// Withdraw from the vault. Does not perform the call to the pool to withdraw the tokens.
///
/// ### Returns
/// * `(i128, i128, i128)` - (
///         The underlying to withdraw from the pool,
///         The amount of b_tokens burned from the vault,
///         the amount of shares burned from the user
///     )
///
/// ### Panics
/// * If the amount is less than or equal to 0
/// * If the user does not have enough shares or bTokens to withdraw
pub fn withdraw(
    e: &Env,
    pool: &Address,
    asset: &Address,
    user: &Address,
    amount: i128,
) -> (i128, i128, i128) {
    let mut vault = get_vault_updated(e, pool, asset);
    let mut user_shares = storage::get_vault_shares(e, user);

    let mut b_tokens_amount = vault.underlying_to_b_tokens_up(amount);
    require_positive(e, b_tokens_amount, BlendVaultError::InvalidBTokensBurnt);
    let mut share_amount = vault.b_tokens_to_shares_up(b_tokens_amount);
    require_positive(e, share_amount, BlendVaultError::InvalidSharesBurnt);
    let mut underlying_amount = amount;

    if share_amount > user_shares {
        // input amount is too high - burn all shares if user has shares to burn
        // round b_token and underlying down to prevent excess withdrawal amounts
        require_positive(e, user_shares, BlendVaultError::BalanceError);
        share_amount = user_shares;
        underlying_amount =
            vault.b_tokens_to_underlying_down(vault.shares_to_b_tokens_down(share_amount));
        // the blend pool will round up the b_tokens burnt based on the underlying amount withdrawn
        b_tokens_amount = vault.underlying_to_b_tokens_up(underlying_amount);
    }

    if vault.total_shares < share_amount || vault.total_b_tokens < b_tokens_amount {
        panic_with_error!(e, BlendVaultError::InsufficientReserves);
    }

    vault.total_shares -= share_amount;
    vault.total_b_tokens -= b_tokens_amount;

    user_shares -= share_amount;
    storage::set_vault_data(e, &vault);
    storage::set_vault_shares(e, user, user_shares);
    (underlying_amount, b_tokens_amount, share_amount)
}

#[cfg(test)]
mod proptests {
    use super::*;
    use crate::testutils::create_test_blend_vault;
    use proptest::prelude::*;
    use soroban_sdk::testutils::Address as _;

    // 100 billion tokens at 7 decimals. Keeps every intermediate product
    // (amount * total, amount * b_rate) well below i128::MAX so the properties
    // probe rounding behavior, not overflow traps.
    const MAX_TOKENS: i128 = 1_000_000_000_000_000_000;
    // 0.5x to 10x — covers default scenarios (b_rate below 1.0) and years of yield
    const MIN_B_RATE: i128 = 500_000_000_000;
    const MAX_B_RATE: i128 = 10_000_000_000_000;

    prop_compose! {
        /// A reachable vault state: either empty (both totals 0, as constructed
        /// or after a full drain) or with both totals positive.
        fn vault_state()(
            total_shares in 1i128..=MAX_TOKENS,
            total_b_tokens in 1i128..=MAX_TOKENS,
            b_rate in MIN_B_RATE..=MAX_B_RATE,
            empty in any::<bool>(),
        ) -> VaultData {
            VaultData {
                last_update_timestamp: 0,
                b_rate,
                total_shares: if empty { 0 } else { total_shares },
                total_b_tokens: if empty { 0 } else { total_b_tokens },
            }
        }
    }

    proptest! {
        #[test]
        fn conversions_never_panic(vault in vault_state(), amount in 0i128..=MAX_TOKENS) {
            vault.b_tokens_to_shares_down(amount);
            vault.b_tokens_to_shares_up(amount);
            vault.shares_to_b_tokens_down(amount);
            vault.b_tokens_to_underlying_down(amount);
            vault.underlying_to_b_tokens_down(amount);
            vault.underlying_to_b_tokens_up(amount);
        }

        /// Minting shares from bTokens and converting back never gains bTokens.
        #[test]
        fn share_round_trip_never_gains(vault in vault_state(), b_tokens in 0i128..=MAX_TOKENS) {
            let shares = vault.b_tokens_to_shares_down(b_tokens);
            prop_assert!(vault.shares_to_b_tokens_down(shares) <= b_tokens);
        }

        /// Converting underlying to bTokens and back never gains underlying.
        #[test]
        fn underlying_round_trip_never_gains(vault in vault_state(), amount in 0i128..=MAX_TOKENS) {
            let b_tokens = vault.underlying_to_b_tokens_down(amount);
            prop_assert!(vault.b_tokens_to_underlying_down(b_tokens) <= amount);
        }

        /// The up and down variants bracket the exact ratio and differ by at most 1.
        #[test]
        fn rounding_up_within_one_of_down(vault in vault_state(), amount in 0i128..=MAX_TOKENS) {
            let shares_down = vault.b_tokens_to_shares_down(amount);
            let shares_up = vault.b_tokens_to_shares_up(amount);
            prop_assert!(shares_down <= shares_up);
            prop_assert!(shares_up - shares_down <= 1);

            let b_tokens_down = vault.underlying_to_b_tokens_down(amount);
            let b_tokens_up = vault.underlying_to_b_tokens_up(amount);
            prop_assert!(b_tokens_down <= b_tokens_up);
            prop_assert!(b_tokens_up - b_tokens_down <= 1);
        }

        /// More shares never convert to fewer bTokens (and likewise for the
        /// other down-rounding conversions).
        #[test]
        fn conversions_monotonic(
            vault in vault_state(),
            a in 0i128..=MAX_TOKENS,
            b in 0i128..=MAX_TOKENS,
        ) {
            let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
            prop_assert!(vault.shares_to_b_tokens_down(lo) <= vault.shares_to_b_tokens_down(hi));
            prop_assert!(vault.b_tokens_to_shares_down(lo) <= vault.b_tokens_to_shares_down(hi));
            prop_assert!(vault.b_tokens_to_underlying_down(lo) <= vault.b_tokens_to_underlying_down(hi));
        }
    }

    prop_compose! {
        /// Like `vault_state`, but with total_shares held within 0.5x-2x of
        /// total_b_tokens. Deposits mint pro-rata, so real vaults never stray
        /// far from 1:1; unbounded ratios would overflow i128 on intermediate
        /// products in states the contract can't actually reach.
        fn proportional_vault_state()(
            total_b_tokens in 1i128..=MAX_TOKENS,
            ratio_bps in 5000i128..=20000,
            b_rate in MIN_B_RATE..=MAX_B_RATE,
            empty in any::<bool>(),
        ) -> VaultData {
            VaultData {
                last_update_timestamp: 0,
                b_rate,
                total_shares: if empty { 0 } else { (total_b_tokens * ratio_bps / 10000).max(1) },
                total_b_tokens: if empty { 0 } else { total_b_tokens },
            }
        }
    }

    proptest! {
        // Each case registers contracts in a fresh Env, so run fewer cases.
        #![proptest_config(ProptestConfig::with_cases(64))]

        /// With an unchanged b_rate, a deposit followed by withdrawing the full
        /// position can never extract more underlying than was deposited —
        /// neither immediately (position value) nor via the withdraw path.
        #[test]
        fn deposit_then_full_withdraw_never_profits(
            initial_state in proportional_vault_state(),
            amount in 1_0000000i128..=MAX_TOKENS,
        ) {
            // skip dust deposits the contract (correctly) rejects
            let b_tokens = initial_state.underlying_to_b_tokens_down(amount);
            prop_assume!(b_tokens > 0);
            prop_assume!(initial_state.b_tokens_to_shares_down(b_tokens) > 0);

            let e = Env::default();
            e.mock_all_auths();
            let bombadil = Address::generate(&e);
            let samwise = Address::generate(&e);
            let (vault_address, pool, asset) =
                create_test_blend_vault(&e, &bombadil, Some(initial_state.b_rate));

            let (position_value, withdrawn, residual_value) =
                e.as_contract(&vault_address, || {
                    storage::set_vault_data(&e, &initial_state);

                    let (_, shares_minted) = deposit(&e, &pool, &asset, &samwise, amount);

                    let v = storage::get_vault_data(&e);
                    let position_value =
                        v.b_tokens_to_underlying_down(v.shares_to_b_tokens_down(shares_minted));

                    if position_value == 0 {
                        return (0, 0, 0);
                    }
                    let (withdrawn, _, _) =
                        withdraw(&e, &pool, &asset, &samwise, position_value);

                    // value of any shares left behind by rounding
                    let v = storage::get_vault_data(&e);
                    let remaining_shares = storage::get_vault_shares(&e, &samwise);
                    let residual_value = v
                        .b_tokens_to_underlying_down(v.shares_to_b_tokens_down(remaining_shares));

                    (position_value, withdrawn, residual_value)
                });

            prop_assert!(position_value <= amount);
            prop_assert!(withdrawn + residual_value <= amount);
        }
    }
}

#[cfg(test)]
mod generic_tests {
    use super::*;
    use crate::testutils::{create_test_blend_vault, mockpool::MockPoolClient, EnvTestUtils};
    use soroban_sdk::{testutils::Address as _, Address};

    #[test]
    fn test_b_tokens_to_shares_down() {
        let mut vault = VaultData {
            b_rate: 1_000_000_000_000,
            last_update_timestamp: 0,
            total_shares: 0,
            total_b_tokens: 0,
        };

        // rounds down
        vault.total_shares = 200_0000001;
        vault.total_b_tokens = 100_0000000;
        let b_tokens = vault.b_tokens_to_shares_down(1_0000000);
        assert_eq!(b_tokens, 2_0000000);

        // returns amount if total_shares is 0
        vault.total_shares = 0;
        vault.total_b_tokens = 100_0000000;
        let b_tokens = vault.b_tokens_to_shares_down(1_0000000);
        assert_eq!(b_tokens, 1_0000000);

        // returns amount if total_b_tokens is 0
        vault.total_shares = 200_0000000;
        vault.total_b_tokens = 0;
        let b_tokens = vault.b_tokens_to_shares_down(1_0000000);
        assert_eq!(b_tokens, 1_0000000);
    }

    #[test]
    fn test_b_tokens_to_shares_up() {
        let mut vault = VaultData {
            b_rate: 1_000_000_000_000,
            last_update_timestamp: 0,
            total_shares: 0,
            total_b_tokens: 0,
        };

        // rounds up
        vault.total_shares = 200_0000001;
        vault.total_b_tokens = 100_0000000;
        let b_tokens = vault.b_tokens_to_shares_up(1_0000000);
        assert_eq!(b_tokens, 2_0000001);

        // returns amount if total_shares is 0
        vault.total_shares = 0;
        vault.total_b_tokens = 100_0000000;
        let b_tokens = vault.b_tokens_to_shares_up(1_0000000);
        assert_eq!(b_tokens, 1_0000000);

        // returns amount if total_b_tokens is 0
        vault.total_shares = 200_0000000;
        vault.total_b_tokens = 0;
        let b_tokens = vault.b_tokens_to_shares_up(1_0000000);
        assert_eq!(b_tokens, 1_0000000);
    }

    #[test]
    fn test_shares_to_b_tokens_down() {
        let mut vault = VaultData {
            b_rate: 1_000_000_000_000,
            last_update_timestamp: 0,
            total_shares: 0,
            total_b_tokens: 0,
        };

        // rounds down
        vault.total_shares = 200_0000001;
        vault.total_b_tokens = 100_0000000;
        let b_tokens = vault.shares_to_b_tokens_down(2_0000000);
        assert_eq!(b_tokens, 0_9999999);

        // returns 0 if total_b_tokens is 0
        vault.total_shares = 200_0000000;
        vault.total_b_tokens = 0;
        let b_tokens = vault.shares_to_b_tokens_down(2_0000000);
        assert_eq!(b_tokens, 0);

        // returns amount 1:1 if total_shares is 0 (empty vault, first deposit rate)
        vault.total_shares = 0;
        vault.total_b_tokens = 0;
        let b_tokens = vault.shares_to_b_tokens_down(2_0000000);
        assert_eq!(b_tokens, 2_0000000);
    }

    #[test]
    fn test_deposit() {
        let e = Env::default();
        e.mock_all_auths();

        let bombadil = Address::generate(&e);
        let samwise = Address::generate(&e);
        let (vault_address, pool, asset) = create_test_blend_vault(&e, &bombadil, None);

        let init_b_rate = 1_100_000_000_000;
        let mock_client = MockPoolClient::new(&e, &pool);
        e.as_contract(&vault_address, || {
            let vault_data = VaultData {
                total_b_tokens: 1000_0000000,
                total_shares: 1200_0000000,
                b_rate: init_b_rate,
                last_update_timestamp: e.ledger().timestamp(),
            };
            storage::set_vault_data(&e, &vault_data);

            // Raise b_rate and deposit — update_rate picks up the new rate, no fee deduction
            let new_b_rate = 1_110_000_000_000;
            mock_client.set_b_rate(&new_b_rate);
            e.jump(5);

            let amount = 100_0000000;
            let expected_b_tokens = amount
                .fixed_div_floor(new_b_rate, SCALAR_12)
                .unwrap_optimized();
            let expected_shares = expected_b_tokens
                .fixed_mul_floor(1200_0000000, 1000_0000000)
                .unwrap_optimized();

            let (b_tokens_minted, shares_minted) = deposit(&e, &pool, &asset, &samwise, amount);
            assert_eq!(b_tokens_minted, expected_b_tokens);
            assert_eq!(shares_minted, expected_shares);

            // All b_tokens go to depositors — no admin deduction
            let new_vault = storage::get_vault_data(&e);
            assert_eq!(new_vault.total_shares, 1200_0000000 + expected_shares);
            assert_eq!(new_vault.total_b_tokens, 1000_0000000 + expected_b_tokens);
            assert_eq!(new_vault.b_rate, new_b_rate);

            let new_balance = storage::get_vault_shares(&e, &samwise);
            assert_eq!(new_balance, expected_shares);
        });
    }

    #[test]
    fn test_initial_deposit() {
        let e = Env::default();
        e.mock_all_auths_allowing_non_root_auth();

        let bombadil = Address::generate(&e);
        let samwise = Address::generate(&e);
        let (vault_address, pool, asset) = create_test_blend_vault(&e, &bombadil, None);

        let init_b_rate = 1_000_000_000_000;
        let mock_client = MockPoolClient::new(&e, &pool);
        e.as_contract(&vault_address, || {
            let vault_data = VaultData {
                total_b_tokens: 0,
                total_shares: 0,
                b_rate: init_b_rate,
                last_update_timestamp: e.ledger().timestamp(),
            };
            storage::set_vault_data(&e, &vault_data);

            // Perform a deposit for samwise
            let new_b_rate = 1_100_000_000_000;
            mock_client.set_b_rate(&new_b_rate);
            e.jump(5);
            let amount = 100_0000000;
            let expected_b_tokens = amount
                .fixed_div_floor(new_b_rate, SCALAR_12)
                .unwrap_optimized();
            let (b_tokens_minted, shares_minted) = deposit(&e, &pool, &asset, &samwise, amount);

            // Load the updated vault to verify the changes
            let expected_share_amount = expected_b_tokens;
            assert_eq!(b_tokens_minted, expected_b_tokens);
            assert_eq!(shares_minted, expected_share_amount);
            let new_vault = storage::get_vault_data(&e);
            assert_eq!(new_vault.total_shares, expected_share_amount);
            assert_eq!(new_vault.total_b_tokens, b_tokens_minted);
            assert_eq!(new_vault.b_rate, new_b_rate);

            let new_balance = storage::get_vault_shares(&e, &samwise);
            assert_eq!(new_balance, expected_share_amount);
        });
    }

    #[test]
    #[should_panic(expected = "Error(Contract, #106)")]
    fn test_deposit_zero_amount() {
        let e = Env::default();
        e.mock_all_auths();

        let bombadil = Address::generate(&e);
        let samwise = Address::generate(&e);
        let (vault_address, pool, asset) = create_test_blend_vault(&e, &bombadil, None);
        e.as_contract(&vault_address, || {
            let vault_data = VaultData {
                total_b_tokens: 1000_0000000,
                total_shares: 1200_0000000,
                b_rate: 1_100_000_000_000,
                last_update_timestamp: e.ledger().timestamp(),
            };
            storage::set_vault_data(&e, &vault_data);

            deposit(&e, &pool, &asset, &samwise, 0);
        });
    }

    #[test]
    #[should_panic(expected = "Error(Contract, #106)")]
    fn test_deposit_zero_b_tokens() {
        let e = Env::default();
        e.mock_all_auths();

        let bombadil = Address::generate(&e);
        let samwise = Address::generate(&e);
        let (vault_address, pool, asset) = create_test_blend_vault(&e, &bombadil, None);

        e.as_contract(&vault_address, || {
            let vault_data = VaultData {
                total_b_tokens: 1000_0000000,
                total_shares: 1200_0000000,
                b_rate: 1_100_000_000_000,
                last_update_timestamp: e.ledger().timestamp(),
            };
            storage::set_vault_data(&e, &vault_data);

            deposit(&e, &pool, &asset, &samwise, 1);
        });
    }

    #[test]
    #[should_panic(expected = "Error(Contract, #108)")]
    fn test_deposit_zero_shares() {
        let e = Env::default();
        e.mock_all_auths();

        let bombadil = Address::generate(&e);
        let samwise = Address::generate(&e);
        let (vault_address, pool, asset) = create_test_blend_vault(&e, &bombadil, None);

        e.as_contract(&vault_address, || {
            // Not possible config in practice, but just in case
            let vault_data = VaultData {
                total_b_tokens: 10000_0000000,
                total_shares: 1200_0000000,
                b_rate: 1_100_000_000_000,
                last_update_timestamp: e.ledger().timestamp(),
            };
            storage::set_vault_data(&e, &vault_data);

            deposit(&e, &pool, &asset, &samwise, 2);
        });
    }

    #[test]
    fn test_withdraw() {
        let e = Env::default();
        e.mock_all_auths();

        let bombadil = Address::generate(&e);
        let samwise = Address::generate(&e);
        let (vault_address, pool, asset) = create_test_blend_vault(&e, &bombadil, None);

        e.as_contract(&vault_address, || {
            let vault_data = VaultData {
                total_b_tokens: 1000_0000000,
                total_shares: 1200_0000000,
                b_rate: 1_100_000_000_000,
                last_update_timestamp: e.ledger().timestamp(),
            };
            storage::set_vault_data(&e, &vault_data);

            // samwise owns all shares
            let sam_shares = 1200_0000000;
            storage::set_vault_shares(&e, &samwise, sam_shares);

            let b_tokens_to_withdraw = 50_0000000;
            let withdraw_amount = vault_data.b_tokens_to_underlying_down(b_tokens_to_withdraw);

            let (underlying_withdrawn, b_tokens_burnt, shares_burnt) =
                withdraw(&e, &pool, &asset, &samwise, withdraw_amount);
            assert_eq!(underlying_withdrawn, withdraw_amount);
            assert_eq!(b_tokens_burnt, b_tokens_to_withdraw);

            let new_vault = storage::get_vault_data(&e);
            assert_eq!(new_vault.total_shares, 1200_0000000 - shares_burnt);
            assert_eq!(new_vault.total_b_tokens, 1000_0000000 - b_tokens_to_withdraw);
            assert_eq!(new_vault.b_rate, 1_100_000_000_000);

            let new_balance = storage::get_vault_shares(&e, &samwise);
            assert_eq!(new_balance, sam_shares - shares_burnt);
        });
    }

    #[test]
    fn test_withdraw_max() {
        let e = Env::default();
        e.mock_all_auths();

        let bombadil = Address::generate(&e);
        let samwise = Address::generate(&e);
        let (vault_address, pool, asset) = create_test_blend_vault(&e, &bombadil, None);

        e.as_contract(&vault_address, || {
            let vault_data = VaultData {
                total_b_tokens: 1000_0000000,
                total_shares: 1200_0000000,
                b_rate: 1_100_000_000_000,
                last_update_timestamp: e.ledger().timestamp(),
            };
            storage::set_vault_data(&e, &vault_data);

            storage::set_vault_shares(&e, &samwise, vault_data.total_shares);
            let withdraw_amount = vault_data.b_tokens_to_underlying_down(1000_0000000);

            let (underlying_withdrawn, b_tokens_burnt, shares_burnt) =
                withdraw(&e, &pool, &asset, &samwise, withdraw_amount);
            assert_eq!(underlying_withdrawn, withdraw_amount);
            assert_eq!(b_tokens_burnt, 1000_0000000);
            assert_eq!(shares_burnt, 1200_0000000);
            let new_balance = storage::get_vault_shares(&e, &samwise);
            assert_eq!(new_balance, 0);
        });
    }

    #[test]
    #[should_panic(expected = "Error(Contract, #107)")]
    fn test_withdraw_zero_amount() {
        let e = Env::default();
        e.mock_all_auths();

        let bombadil = Address::generate(&e);
        let samwise = Address::generate(&e);
        let (vault_address, pool, asset) = create_test_blend_vault(&e, &bombadil, None);

        e.as_contract(&vault_address, || {
            let vault_data = VaultData {
                total_b_tokens: 1000_0000000,
                total_shares: 1200_0000000,
                b_rate: 1_100_000_000_000,
                last_update_timestamp: e.ledger().timestamp(),
            };
            storage::set_vault_data(&e, &vault_data);

            withdraw(&e, &pool, &asset, &samwise, 0);
        });
    }

    #[test]
    #[should_panic(expected = "Error(Contract, #105)")]
    fn test_withdraw_more_b_tokens_than_vault() {
        let e = Env::default();
        e.mock_all_auths();

        let bombadil = Address::generate(&e);
        let samwise = Address::generate(&e);
        let (vault_address, pool, asset) = create_test_blend_vault(&e, &bombadil, None);

        e.as_contract(&vault_address, || {
            let vault_data = VaultData {
                total_b_tokens: 1000_0000000,
                total_shares: 1200_0000000,
                b_rate: 1_100_000_000_000,
                last_update_timestamp: e.ledger().timestamp(),
            };
            storage::set_vault_data(&e, &vault_data);

            storage::set_vault_shares(&e, &samwise, vault_data.total_shares + 10);
            let withdraw_amount = vault_data.b_tokens_to_underlying_down(1000_0000000);

            withdraw(&e, &pool, &asset, &samwise, withdraw_amount + 1);
        });
    }

    #[test]
    fn test_withdraw_exact_balance() {
        let e = Env::default();
        e.mock_all_auths();

        let bombadil = Address::generate(&e);
        let samwise = Address::generate(&e);
        let (vault_address, pool, asset) = create_test_blend_vault(&e, &bombadil, None);

        e.as_contract(&vault_address, || {
            let vault_data = VaultData {
                total_b_tokens: 1000_0000000,
                total_shares: 1200_0000000,
                b_rate: 1_100_000_000_000,
                last_update_timestamp: e.ledger().timestamp(),
            };
            storage::set_vault_data(&e, &vault_data);

            let sam_shares = 1000_0000000;
            storage::set_vault_shares(&e, &samwise, sam_shares);
            let sam_b_tokens: i128 =
                vault_data.shares_to_b_tokens_down(storage::get_vault_shares(&e, &samwise));
            let sam_underlying_balance = vault_data.b_tokens_to_underlying_down(sam_b_tokens);

            // Withdraw whole underlying balance as read by the contract
            let (underlying_withdrawn, b_tokens_burnt, shares_burnt) =
                withdraw(&e, &pool, &asset, &samwise, sam_underlying_balance);
            assert_eq!(underlying_withdrawn, sam_underlying_balance);
            assert_eq!(b_tokens_burnt, sam_b_tokens);
            assert_eq!(shares_burnt, sam_shares);
            assert_eq!(storage::get_vault_shares(&e, &samwise), 0);
        });
    }

    #[test]
    fn test_withdraw_over_balance() {
        let e = Env::default();
        e.mock_all_auths();

        let bombadil = Address::generate(&e);
        let samwise = Address::generate(&e);
        let (vault_address, pool, asset) = create_test_blend_vault(&e, &bombadil, None);

        e.as_contract(&vault_address, || {
            let vault_data = VaultData {
                total_b_tokens: 1000_0000000,
                total_shares: 1200_0000000,
                b_rate: 1_100_000_000_000,
                last_update_timestamp: e.ledger().timestamp(),
            };
            storage::set_vault_data(&e, &vault_data);

            let sam_shares = 1000_0000000;
            storage::set_vault_shares(&e, &samwise, sam_shares);
            let sam_b_tokens: i128 =
                vault_data.shares_to_b_tokens_down(storage::get_vault_shares(&e, &samwise));
            let sam_underlying_balance = vault_data.b_tokens_to_underlying_down(sam_b_tokens);

            // Try to withdraw 1 more than `sam_underlying_balance`
            let (underlying_withdrawn, b_tokens_burnt, shares_burnt) =
                withdraw(&e, &pool, &asset, &samwise, sam_underlying_balance + 1);
            // Pulls back down to `sam_underlying_balance`
            assert_eq!(underlying_withdrawn, sam_underlying_balance);
            assert_eq!(b_tokens_burnt, sam_b_tokens);
            assert_eq!(shares_burnt, sam_shares);
            assert_eq!(storage::get_vault_shares(&e, &samwise), 0);
        });
    }

    #[test]
    fn test_withdraw_over_balance_full_vault() {
        let e = Env::default();
        e.mock_all_auths();

        let bombadil = Address::generate(&e);
        let samwise = Address::generate(&e);
        let (vault_address, pool, asset) = create_test_blend_vault(&e, &bombadil, None);

        e.as_contract(&vault_address, || {
            let vault_data = VaultData {
                total_b_tokens: 1000_0000000,
                total_shares: 1200_0000000,
                b_rate: 1_100_000_000_000,
                last_update_timestamp: e.ledger().timestamp(),
            };
            storage::set_vault_data(&e, &vault_data);

            let sam_shares = 1200_0000000;
            storage::set_vault_shares(&e, &samwise, sam_shares);
            let sam_b_tokens: i128 =
                vault_data.shares_to_b_tokens_down(storage::get_vault_shares(&e, &samwise));
            let sam_underlying_balance = vault_data.b_tokens_to_underlying_down(sam_b_tokens);

            let (underlying_withdrawn, b_tokens_burnt, shares_burnt) =
                withdraw(&e, &pool, &asset, &samwise, i64::MAX as i128);
            // Pulls back down to `sam_underlying_balance`
            assert_eq!(underlying_withdrawn, sam_underlying_balance);
            assert_eq!(b_tokens_burnt, sam_b_tokens);
            assert_eq!(shares_burnt, sam_shares);
            assert_eq!(storage::get_vault_shares(&e, &samwise), 0);
            let vault_data = storage::get_vault_data(&e);
            assert_eq!(vault_data.total_b_tokens, 0);
            assert_eq!(vault_data.total_shares, 0);
        });
    }

    #[test]
    fn test_update_rate() {
        let e = Env::default();
        e.mock_all_auths();
        e.set_default_info();

        let init_b_rate = 1_100_000_000_000;
        let bombadil = Address::generate(&e);
        let (vault_address, pool, asset) = create_test_blend_vault(&e, &bombadil, Some(init_b_rate));

        let mock_client = MockPoolClient::new(&e, &pool);

        e.as_contract(&vault_address, || {
            let mut vault_data = VaultData {
                total_b_tokens: 1000_0000000,
                last_update_timestamp: e.ledger().timestamp(),
                total_shares: 1200_0000000,
                b_rate: init_b_rate,
            };

            // update b_rate to 1.2
            let new_b_rate = 120_000_0000_000;
            mock_client.set_b_rate(&new_b_rate);
            e.jump(5);
            vault_data.update_rate(&e, &pool, &asset);

            // No fees, so b_tokens and shares are unchanged
            assert_eq!(vault_data.total_shares, 1200_000_0000);
            assert_eq!(vault_data.total_b_tokens, 1000_0000000);
            assert_eq!(vault_data.b_rate, new_b_rate);
            assert_eq!(vault_data.last_update_timestamp, e.ledger().timestamp());
        });
    }

    #[test]
    fn test_update_rate_no_change() {
        let e = Env::default();
        e.mock_all_auths();

        let init_b_rate = 1_100_000_000_000;
        let bombadil = Address::generate(&e);
        let (vault_address, pool, asset) = create_test_blend_vault(&e, &bombadil, Some(init_b_rate));

        e.as_contract(&vault_address, || {
            let now = e.ledger().timestamp();
            let mut vault_data = VaultData {
                total_b_tokens: 1000_0000000,
                total_shares: 1200_0000000,
                b_rate: init_b_rate,
                last_update_timestamp: now,
            };

            vault_data.update_rate(&e, &pool, &asset);
            assert_eq!(vault_data.total_shares, 1200_0000000);
            assert_eq!(vault_data.total_b_tokens, 1000_0000000);
            assert_eq!(vault_data.b_rate, init_b_rate);
            assert_eq!(vault_data.last_update_timestamp, now);
        });
    }

    #[test]
    fn test_update_rate_different_timestamp_same_brate() {
        let e = Env::default();
        e.mock_all_auths();

        let init_b_rate = 1_100_000_000_000;
        let bombadil = Address::generate(&e);
        let (vault_address, pool, asset) = create_test_blend_vault(&e, &bombadil, Some(init_b_rate));

        e.as_contract(&vault_address, || {
            let now = e.ledger().timestamp();
            let mut vault_data = VaultData {
                total_b_tokens: 1000_0000000,
                total_shares: 1200_0000000,
                b_rate: init_b_rate,
                last_update_timestamp: now,
            };

            e.jump_time(100);

            vault_data.update_rate(&e, &pool, &asset);
            assert_eq!(vault_data.total_shares, 1200_0000000);
            assert_eq!(vault_data.total_b_tokens, 1000_0000000);
            assert_eq!(vault_data.b_rate, init_b_rate);
            // Timestamp gets updated even when b_rate is unchanged
            assert_eq!(vault_data.last_update_timestamp, e.ledger().timestamp());
        });
    }

    #[test]
    fn test_update_rate_decrease() {
        let e = Env::default();
        e.mock_all_auths();
        e.set_default_info();

        let init_b_rate = 1_100_000_000_000;
        let bombadil = Address::generate(&e);
        let (vault_address, pool, asset) = create_test_blend_vault(&e, &bombadil, Some(init_b_rate));
        let mock_client = MockPoolClient::new(&e, &pool);

        e.as_contract(&vault_address, || {
            let mut vault_data = VaultData {
                total_b_tokens: 100_0000000,
                last_update_timestamp: e.ledger().timestamp(),
                total_shares: 100_0000000,
                b_rate: init_b_rate,
            };

            // b_rate decreases (e.g. in a default scenario)
            let new_b_rate: i128 = 1_050_000_000_000;
            mock_client.set_b_rate(&new_b_rate);
            vault_data.update_rate(&e, &pool, &asset);

            assert_eq!(vault_data.b_rate, new_b_rate);
            assert_eq!(vault_data.total_b_tokens, 100_0000000);
            assert_eq!(vault_data.total_shares, 100_0000000);
            assert_eq!(vault_data.last_update_timestamp, e.ledger().timestamp());
        });
    }
}