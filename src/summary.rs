use blend_contract_sdk::pool::Client as PoolClient;
use soroban_fixed_point_math::SorobanFixedPoint;
use soroban_sdk::{contracttype, Address, Env};

use crate::{
    constants::{SCALAR_12, SCALAR_7},
    storage,
    vault::{self, VaultData},
};

/**
 * @dev
 *
 * Summary of the vault state. Intended for offchain services like a dApp to easily display
 * information about the vault. Not intended for onchain logic.
 */

#[derive(Clone)]
#[contracttype]
pub struct VaultSummary {
    pub pool: Address,
    pub asset: Address,
    pub admin: Address,
    pub signer: Option<Address>,
    pub vault: VaultData,
    pub est_apr: i128,
}

impl VaultSummary {
    pub fn load(e: &Env) -> Self {
        let pool = storage::get_pool(e);
        let asset = storage::get_asset(e);
        let admin = storage::get_admin(e);
        let signer = storage::get_signer(e);
        let vault = vault::get_vault_updated(e, &pool, &asset);

        let reserve = PoolClient::new(e, &pool).get_reserve(&asset);
        let pool_config = PoolClient::new(e, &pool).get_config();

        // calc estimated APR for reserve in the vault
        // code pulled from https://github.com/blend-capital/blend-contracts-v2/blob/main/pool/src/pool/interest.rs#L23
        let liabilities = reserve
            .data
            .d_supply
            .fixed_mul_ceil(e, &reserve.data.d_rate, &SCALAR_12);
        let supply = reserve
            .data
            .b_supply
            .fixed_mul_floor(e, &reserve.data.b_rate, &SCALAR_12);
        let cur_util: i128 = if liabilities == 0 {
            0
        } else if liabilities >= supply {
            SCALAR_7
        } else {
            liabilities.fixed_div_ceil(e, &supply, &SCALAR_7)
        };
        let cur_ir: i128;
        let target_util: i128 = reserve.config.util as i128;
        if cur_util <= target_util {
            let util_scalar = cur_util.fixed_div_ceil(e, &target_util, &SCALAR_7);
            let base_rate =
                util_scalar.fixed_mul_ceil(e, &(reserve.config.r_one as i128), &SCALAR_7)
                    + (reserve.config.r_base as i128);

            cur_ir = base_rate.fixed_mul_ceil(e, &reserve.data.ir_mod, &SCALAR_7);
        } else if cur_util <= 0_9500000 {
            let util_scalar =
                (cur_util - target_util).fixed_div_ceil(e, &(0_9500000 - target_util), &SCALAR_7);
            let base_rate =
                util_scalar.fixed_mul_ceil(e, &(reserve.config.r_two as i128), &SCALAR_7)
                    + (reserve.config.r_one as i128)
                    + (reserve.config.r_base as i128);

            cur_ir = base_rate.fixed_mul_ceil(e, &reserve.data.ir_mod, &SCALAR_7);
        } else {
            let util_scalar = (cur_util - 0_9500000).fixed_div_ceil(e, &0_0500000, &SCALAR_7);
            let extra_rate =
                util_scalar.fixed_mul_ceil(e, &(reserve.config.r_three as i128), &SCALAR_7);

            let intersection = reserve.data.ir_mod.fixed_mul_ceil(
                e,
                &((reserve.config.r_two + reserve.config.r_one + reserve.config.r_base) as i128),
                &SCALAR_7,
            );
            cur_ir = extra_rate + intersection;
        }

        // cur_ir is the borrow rate; convert to supply rate suppliers earn
        let supply_apr = cur_ir
            .fixed_mul_floor(e, &cur_util, &SCALAR_7)
            .fixed_mul_floor(e, &(SCALAR_7 - (pool_config.bstop_rate as i128)), &SCALAR_7);

        VaultSummary {
            pool,
            asset,
            admin,
            signer,
            vault,
            est_apr: supply_apr,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutils::{
        assert_approx_eq_abs,
        mockpool::{register_mock_pool_with_config_and_data, ReserveConfig, ReserveData},
        register_fee_vault, EnvTestUtils,
    };
    use soroban_sdk::testutils::Address as _;

    #[test]
    fn test_vault_summary() {
        let e = Env::default();
        e.cost_estimate().budget().reset_unlimited();
        e.mock_all_auths();
        e.set_default_info();

        let bombadil = Address::generate(&e);
        let token = Address::generate(&e);

        let backstop_rate: u32 = 0_100_0000; // 10%
        let reserve_config = ReserveConfig {
            c_factor: 900_0000,
            decimals: 7,
            index: 0,
            l_factor: 900_0000,
            max_util: 900_0000,
            reactivity: 0,
            r_base: 30_0000,
            r_one: 60_0000,
            r_two: 120_0000,
            r_three: 5_000_0000,
            util: 0_800_0000,
            supply_cap: i64::MAX as i128,
            enabled: true,
        };
        // 85% util, 2.5x ir mod → borrow ir ~32.5% → supply apr ~0.325 * 0.85 * 0.9 ≈ 24.9%
        let reserve_data = ReserveData {
            b_supply: 100_0000000,
            b_rate: 1_500_000_000_000,
            d_supply: 63_7500000,
            d_rate: 2_000_000_000_000,
            ir_mod: 2_500_0000,
            backstop_credit: 0,
            last_time: e.ledger().timestamp(),
        };
        let pool_client = register_mock_pool_with_config_and_data(
            &e,
            backstop_rate,
            reserve_config,
            reserve_data,
        );

        let fee_vault = register_fee_vault(&e, &bombadil, &pool_client.address, &token, None, None);

        e.as_contract(&fee_vault, || {
            let vault_data = VaultData {
                total_b_tokens: 1000_0000000,
                total_shares: 1200_0000000,
                b_rate: 1_500_000_000_000,
                last_update_timestamp: e.ledger().timestamp(),
            };
            storage::set_vault_data(&e, &vault_data);

            let summary = VaultSummary::load(&e);
            assert_eq!(summary.pool, pool_client.address);
            assert_eq!(summary.asset, token);
            assert_eq!(summary.admin, bombadil);
            assert_eq!(summary.signer, None);
            assert_eq!(summary.vault.total_b_tokens, 1000_0000000);
            assert_eq!(summary.vault.total_shares, 1200_0000000);
            assert_eq!(summary.vault.b_rate, 1_500_000_000_000);
            assert_eq!(summary.vault.last_update_timestamp, e.ledger().timestamp());
            // 0.325 * 0.85 * (1 - 0.1) = 0.248625
            assert_approx_eq_abs(summary.est_apr, 0_2486250, 0_0001000);
        });
    }
}