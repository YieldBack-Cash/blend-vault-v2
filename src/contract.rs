use crate::{
    errors::FeeVaultError,
    events::FeeVaultEvents,
    pool, storage, swap,
    summary::VaultSummary,
    validator::require_positive,
    vault::{self, VaultData},
};

use soroban_sdk::{
    auth::{ContractContext, InvokerContractAuthEntry, SubContractInvocation},
    contract, contractimpl, panic_with_error, vec, Address, Env, IntoVal, String, Symbol,
};

#[contract]
pub struct FeeVault;

#[contractimpl]
impl FeeVault {
    /// Initialize the contract
    ///
    /// ### Arguments
    /// * `admin` - The admin address
    /// * `pool` - The blend pool address the vault will deposit into
    /// * `asset` - The asset address of the reserve the vault will support
    /// * `signer`- The signer address if the vault is permissioned, None otherwise
    pub fn __constructor(
        e: Env,
        admin: Address,
        pool: Address,
        asset: Address,
        signer: Option<Address>,
        router: Option<Address>,
    ) {
        admin.require_auth();

        storage::set_admin(&e, admin);
        storage::set_pool(&e, pool.clone());
        storage::set_asset(&e, asset.clone());

        if let Some(signer) = signer {
            storage::set_signer(&e, signer);
        }
        if let Some(router) = router {
            storage::set_router(&e, router);
        }
        storage::set_vault_data(
            &e,
            &VaultData {
                b_rate: pool::reserve_b_rate(&e, &pool, &asset),
                last_update_timestamp: e.ledger().timestamp(),
                total_shares: 0,
                total_b_tokens: 0,
            },
        );
    }

    //********** Read-Only ***********//

    /// Fetch a user's position in shares
    pub fn get_shares(e: Env, user: Address) -> i128 {
        storage::get_vault_shares(&e, &user)
    }

    /// Fetch a user's position in bTokens
    pub fn get_b_tokens(e: Env, user: Address) -> i128 {
        let shares = storage::get_vault_shares(&e, &user);
        if shares > 0 {
            let pool = storage::get_pool(&e);
            let asset = storage::get_asset(&e);
            let vault = vault::get_vault_updated(&e, &pool, &asset);
            vault.shares_to_b_tokens_down(shares)
        } else {
            0
        }
    }

    /// Convert a share amount to underlying tokens
    pub fn convert_to_assets(e: Env, shares: i128) -> i128 {
        if shares <= 0 {
            return 0;
        }
        let pool = storage::get_pool(&e);
        let asset = storage::get_asset(&e);
        let vault = vault::get_vault_updated(&e, &pool, &asset);
        let b_tokens = vault.shares_to_b_tokens_down(shares);
        vault.b_tokens_to_underlying_down(b_tokens)
    }

    /// Fetch a user's position in underlying tokens
    pub fn get_underlying_tokens(e: Env, user: Address) -> i128 {
        let shares = storage::get_vault_shares(&e, &user);
        if shares > 0 {
            let pool = storage::get_pool(&e);
            let asset = storage::get_asset(&e);
            let vault = vault::get_vault_updated(&e, &pool, &asset);
            let b_tokens = vault.shares_to_b_tokens_down(shares);
            vault.b_tokens_to_underlying_down(b_tokens)
        } else {
            0
        }
    }

    /// Get the vault's blend pool and asset addresses
    pub fn get_config(e: Env) -> (Address, Address) {
        (storage::get_pool(&e), storage::get_asset(&e))
    }

    /// Get the vault data
    pub fn get_vault(e: Env) -> VaultData {
        let pool = storage::get_pool(&e);
        let asset = storage::get_asset(&e);
        vault::get_vault_updated(&e, &pool, &asset)
    }

    /// Get the vault's admin
    pub fn get_admin(e: Env) -> Address {
        storage::get_admin(&e)
    }

    /// Get the vault's signer
    pub fn get_signer(e: Env) -> Option<Address> {
        storage::get_signer(&e)
    }

    /// NOT INTENDED FOR CONTRACT USE
    ///
    /// Get the vault summary for dApp display purposes.
    pub fn get_vault_summary(e: Env) -> VaultSummary {
        VaultSummary::load(&e)
    }

    //********** SEP-41 Token Interface ***********//

    /// Returns the share balance of `id`
    pub fn balance(e: Env, id: Address) -> i128 {
        storage::get_vault_shares(&e, &id)
    }

    /// Returns the number of decimals used by vault shares
    pub fn decimals(_e: Env) -> u32 {
        7
    }

    /// Returns the name of the vault share token
    pub fn name(e: Env) -> String {
        String::from_str(&e, "Blend Vault Share")
    }

    /// Returns the symbol of the vault share token
    pub fn symbol(e: Env) -> String {
        String::from_str(&e, "bVS")
    }

    /// Returns the approved allowance for `spender` to transfer shares from `from`
    pub fn allowance(e: Env, from: Address, spender: Address) -> i128 {
        storage::get_allowance(&e, &from, &spender)
    }

    /// Approve `spender` to transfer up to `amount` shares from `from` until `expiration_ledger`
    pub fn approve(e: Env, from: Address, spender: Address, amount: i128, expiration_ledger: u32) {
        from.require_auth();
        storage::set_allowance(&e, &from, &spender, amount, expiration_ledger);
        FeeVaultEvents::approve(&e, &from, &spender, amount, expiration_ledger);
    }

    /// Transfer `amount` shares from `from` to `to`.
    pub fn transfer(e: Env, from: Address, to: Address, amount: i128) {
        from.require_auth();
        require_positive(&e, amount, FeeVaultError::InvalidAmount);

        let from_shares = storage::get_vault_shares(&e, &from);
        if from_shares < amount {
            panic_with_error!(&e, FeeVaultError::BalanceError);
        }
        let to_shares = storage::get_vault_shares(&e, &to);

        storage::set_vault_shares(&e, &from, from_shares - amount);
        storage::set_vault_shares(&e, &to, to_shares + amount);

        FeeVaultEvents::transfer(&e, &from, &to, amount);
    }

    /// Transfer `amount` shares from `from` to `to` using `spender`'s allowance.
    pub fn transfer_from(e: Env, spender: Address, from: Address, to: Address, amount: i128) {
        spender.require_auth();
        require_positive(&e, amount, FeeVaultError::InvalidAmount);

        let (allowance, expiration) = storage::get_allowance_with_expiration(&e, &from, &spender);
        if allowance < amount {
            panic_with_error!(&e, FeeVaultError::BalanceError);
        }

        let from_shares = storage::get_vault_shares(&e, &from);
        if from_shares < amount {
            panic_with_error!(&e, FeeVaultError::BalanceError);
        }
        let to_shares = storage::get_vault_shares(&e, &to);

        storage::set_allowance(&e, &from, &spender, allowance - amount, expiration);

        storage::set_vault_shares(&e, &from, from_shares - amount);
        storage::set_vault_shares(&e, &to, to_shares + amount);

        FeeVaultEvents::transfer(&e, &from, &to, amount);
    }

    //********** Read-Write Admin Only ***********//

    /// ADMIN ONLY
    /// Sets the admin address for the fee vault. Requires a signature from both the current admin
    /// and the new admin address.
    pub fn set_admin(e: Env, admin: Address) {
        storage::extend_instance(&e);
        storage::get_admin(&e).require_auth();
        admin.require_auth();
        storage::set_admin(&e, admin);
    }

    /// ADMIN ONLY
    /// Sets the signer for the fee vault.
    /// Passing `None` will remove the signer requirement for deposits.
    pub fn set_signer(e: Env, signer: Option<Address>) {
        storage::extend_instance(&e);
        storage::get_admin(&e).require_auth();
        if let Some(signer_addr) = signer {
            signer_addr.require_auth();
            storage::set_signer(&e, signer_addr);
        } else {
            storage::del_signer(&e);
        }
    }

    /// ADMIN ONLY
    /// Sets the Soroswap router address used for harvesting BLND emissions.
    ///
    /// ### Arguments
    /// * `router` - The Soroswap router contract address
    pub fn set_router(e: Env, router: Address) {
        storage::extend_instance(&e);
        storage::get_admin(&e).require_auth();
        storage::set_router(&e, router);
    }

    /// Claims accrued BLND emissions from the pool, swaps them for the underlying
    /// asset via Soroswap, and supplies the proceeds back into the pool. Every
    /// depositor's share value increases automatically — no manual distribution needed.
    ///
    /// ### Arguments
    /// * `amount_out_min` - Minimum underlying tokens to accept from the swap (slippage floor)
    ///
    /// ### Returns
    /// * `i128` - The amount of underlying tokens received and re-supplied
    pub fn claim_emissions(e: Env, amount_out_min: i128) -> i128 {
        storage::extend_instance(&e);
        let pool = storage::get_pool(&e);
        let asset = storage::get_asset(&e);
        let blnd = storage::get_blnd_token(&e);
        let router = storage::get_router(&e)
            .unwrap_or_else(|| panic_with_error!(&e, FeeVaultError::SwapNotConfigured));

        let supply_token_id = pool::reserve_supply_token_id(&e, &pool, &asset);
        let blnd_claimed = pool::claim(
            &e,
            &pool,
            &vec![&e, supply_token_id],
            &e.current_contract_address(),
        );
        if blnd_claimed == 0 {
            return 0;
        }

        let underlying_received =
            swap::swap_blnd_for_asset(&e, &router, &blnd, &asset, blnd_claimed, amount_out_min);

        let vault = e.current_contract_address();
        e.authorize_as_current_contract(vec![
            &e,
            InvokerContractAuthEntry::Contract(SubContractInvocation {
                context: ContractContext {
                    contract: asset.clone(),
                    fn_name: Symbol::new(&e, "transfer"),
                    args: (vault.clone(), pool.clone(), underlying_received).into_val(&e),
                },
                sub_invocations: vec![&e],
            }),
        ]);
        pool::supply(&e, &pool, &asset, &vault, underlying_received);

        let mut vault = vault::get_vault_updated(&e, &pool, &asset);
        vault.total_b_tokens =
            pool::vault_b_token_balance(&e, &pool, &asset, &e.current_contract_address());
        storage::set_vault_data(&e, &vault);

        FeeVaultEvents::vault_emissions_claim(&e, &pool, blnd_claimed, underlying_received);
        underlying_received
    }

    //********** Read-Write ***********//

    /// Deposits tokens into the fee vault. Requires the signer to sign if one is set.
    ///
    /// ### Arguments
    /// * `assets` - The amount of underlying tokens to deposit
    /// * `receiver` - The address that will receive the minted vault shares
    /// * `from` - The address providing the underlying tokens
    /// * `operator` - The address initiating the deposit
    ///
    /// ### Returns
    /// * `i128` - The number of shares minted to the receiver
    pub fn deposit(e: Env, assets: i128, receiver: Address, from: Address, operator: Address) -> i128 {
        storage::extend_instance(&e);
        operator.require_auth();
        if let Some(signer) = storage::get_signer(&e) {
            signer.require_auth();
        }

        require_positive(&e, assets, FeeVaultError::InvalidAmount);

        let pool = storage::get_pool(&e);
        let asset = storage::get_asset(&e);
        pool::supply(&e, &pool, &asset, &from, assets);
        let (b_tokens_minted, new_shares) = vault::deposit(&e, &pool, &asset, &receiver, assets);

        FeeVaultEvents::vault_deposit(
            &e,
            &pool,
            &asset,
            &from,
            assets,
            new_shares,
            b_tokens_minted,
        );
        new_shares
    }

    /// Withdraws tokens from the fee vault. If the input amount exceeds the owner's
    /// balance, the owner's full balance will be withdrawn.
    ///
    /// ### Arguments
    /// * `assets` - The amount of underlying tokens to withdraw
    /// * `receiver` - The address that will receive the withdrawn tokens
    /// * `owner` - The address whose vault shares will be burned
    /// * `operator` - The address initiating the withdrawal
    ///
    /// ### Returns
    /// * `i128` - The number of shares burnt
    pub fn withdraw(e: Env, assets: i128, receiver: Address, owner: Address, operator: Address) -> i128 {
        storage::extend_instance(&e);
        operator.require_auth();
        require_positive(&e, assets, FeeVaultError::InvalidAmount);

        let pool = storage::get_pool(&e);
        let asset = storage::get_asset(&e);
        let (withdraw_amount, b_tokens_burnt, burnt_shares) =
            vault::withdraw(&e, &pool, &asset, &owner, assets);
        pool::withdraw(&e, &pool, &asset, &receiver, withdraw_amount);

        FeeVaultEvents::vault_withdraw(
            &e,
            &pool,
            &asset,
            &owner,
            assets,
            burnt_shares,
            b_tokens_burnt,
        );
        burnt_shares
    }

}