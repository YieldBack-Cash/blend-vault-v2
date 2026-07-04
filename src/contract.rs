use crate::{
    errors::BlendVaultError,
    events::BlendVaultEvents,
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
pub struct BlendVault;

#[contractimpl]
impl BlendVault {
    /// Initializes the contract with an admin, blend pool, and asset.
    ///
    /// ### Arguments
    /// * `admin` - The admin address
    /// * `pool` - The Blend pool the vault will deposit into
    /// * `asset` - The reserve asset the vault supports
    pub fn __constructor(
        e: Env,
        admin: Address,
        pool: Address,
        asset: Address,
        blnd_token: Address,
    ) {
        admin.require_auth();

        storage::set_admin(&e, admin);
        storage::set_pool(&e, pool.clone());
        storage::set_asset(&e, asset.clone());
        storage::set_blnd_token(&e, blnd_token);

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

    /// Returns `user`'s position in shares.
    ///
    /// ### Arguments
    /// * `user` - The address to query
    pub fn get_shares(e: Env, user: Address) -> i128 {
        storage::get_vault_shares(&e, &user)
    }

    /// Returns `user`'s position in bTokens.
    ///
    /// ### Arguments
    /// * `user` - The address to query
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

    /// Converts a share amount to underlying tokens.
    ///
    /// ### Arguments
    /// * `shares` - The share amount to convert
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

    /// Returns `user`'s position in underlying tokens.
    ///
    /// ### Arguments
    /// * `user` - The address to query
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

    /// Returns the vault's blend pool and asset addresses.
    pub fn get_config(e: Env) -> (Address, Address) {
        (storage::get_pool(&e), storage::get_asset(&e))
    }

    /// Returns the current vault data with an up-to-date bRate.
    pub fn get_vault(e: Env) -> VaultData {
        let pool = storage::get_pool(&e);
        let asset = storage::get_asset(&e);
        vault::get_vault_updated(&e, &pool, &asset)
    }

    /// Returns the vault's admin address.
    pub fn get_admin(e: Env) -> Address {
        storage::get_admin(&e)
    }

    /// Returns the vault's signer address, or `None` if no signer is set.
    pub fn get_signer(e: Env) -> Option<Address> {
        storage::get_signer(&e)
    }

    /// Returns a vault summary for offchain display (e.g. dApp). Not for onchain use.
    pub fn get_vault_summary(e: Env) -> VaultSummary {
        VaultSummary::load(&e)
    }

    //********** SEP-41 Token Interface ***********//

    /// Returns the share balance of `id`.
    ///
    /// ### Arguments
    /// * `id` - The address to query
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

    /// Returns the approved allowance for `spender` to transfer shares from `from`.
    ///
    /// ### Arguments
    /// * `from` - The address owning the shares
    /// * `spender` - The address approved to spend
    pub fn allowance(e: Env, from: Address, spender: Address) -> i128 {
        storage::get_allowance(&e, &from, &spender)
    }

    /// Approves `spender` to transfer up to `amount` shares from `from` until `expiration_ledger`.
    ///
    /// ### Arguments
    /// * `from` - The address owning the shares
    /// * `spender` - The address being approved
    /// * `amount` - The maximum shares the spender may transfer
    /// * `expiration_ledger` - The ledger sequence at which the approval expires
    pub fn approve(e: Env, from: Address, spender: Address, amount: i128, expiration_ledger: u32) {
        from.require_auth();
        storage::set_allowance(&e, &from, &spender, amount, expiration_ledger);
        BlendVaultEvents::approve(&e, &from, &spender, amount, expiration_ledger);
    }

    /// Transfers `amount` shares from `from` to `to`.
    ///
    /// ### Arguments
    /// * `from` - The address sending shares
    /// * `to` - The address receiving shares
    /// * `amount` - The number of shares to transfer
    pub fn transfer(e: Env, from: Address, to: Address, amount: i128) {
        from.require_auth();
        require_positive(&e, amount, BlendVaultError::InvalidAmount);

        let from_shares = storage::get_vault_shares(&e, &from);
        if from_shares < amount {
            panic_with_error!(&e, BlendVaultError::BalanceError);
        }
        let to_shares = storage::get_vault_shares(&e, &to);

        storage::set_vault_shares(&e, &from, from_shares - amount);
        storage::set_vault_shares(&e, &to, to_shares + amount);

        BlendVaultEvents::transfer(&e, &from, &to, amount);
    }

    /// Transfers `amount` shares from `from` to `to` using `spender`'s allowance.
    ///
    /// ### Arguments
    /// * `spender` - The address whose allowance is consumed
    /// * `from` - The address sending shares
    /// * `to` - The address receiving shares
    /// * `amount` - The number of shares to transfer
    pub fn transfer_from(e: Env, spender: Address, from: Address, to: Address, amount: i128) {
        spender.require_auth();
        require_positive(&e, amount, BlendVaultError::InvalidAmount);

        let (allowance, expiration) = storage::get_allowance_with_expiration(&e, &from, &spender);
        if allowance < amount {
            panic_with_error!(&e, BlendVaultError::BalanceError);
        }

        let from_shares = storage::get_vault_shares(&e, &from);
        if from_shares < amount {
            panic_with_error!(&e, BlendVaultError::BalanceError);
        }
        let to_shares = storage::get_vault_shares(&e, &to);

        storage::set_allowance(&e, &from, &spender, allowance - amount, expiration);

        storage::set_vault_shares(&e, &from, from_shares - amount);
        storage::set_vault_shares(&e, &to, to_shares + amount);

        BlendVaultEvents::transfer(&e, &from, &to, amount);
    }

    //********** Read-Write Admin Only ***********//

    /// Sets the admin address. Requires auth from both the current and new admin.
    ///
    /// ### Arguments
    /// * `admin` - The new admin address
    pub fn set_admin(e: Env, admin: Address) {
        storage::extend_instance(&e);
        storage::get_admin(&e).require_auth();
        admin.require_auth();
        storage::set_admin(&e, admin);
    }

    /// Sets the deposit signer. Pass `None` to remove the signer requirement.
    ///
    /// ### Arguments
    /// * `signer` - The new signer address, or `None` to disable
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
            .unwrap_or_else(|| panic_with_error!(&e, BlendVaultError::SwapNotConfigured));

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

        BlendVaultEvents::vault_emissions_claim(&e, &pool, blnd_claimed, underlying_received);
        underlying_received
    }

    //********** Read-Write ***********//

    /// Deposits tokens into the blend vault. Requires the signer to sign if one is set.
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

        require_positive(&e, assets, BlendVaultError::InvalidAmount);

        let pool = storage::get_pool(&e);
        let asset = storage::get_asset(&e);
        pool::supply(&e, &pool, &asset, &from, assets);
        let (b_tokens_minted, new_shares) = vault::deposit(&e, &pool, &asset, &receiver, assets);

        BlendVaultEvents::vault_deposit(
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

    /// Withdraws tokens from the blend vault. If the input amount exceeds the owner's
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
        require_positive(&e, assets, BlendVaultError::InvalidAmount);

        let pool = storage::get_pool(&e);
        let asset = storage::get_asset(&e);
        let (withdraw_amount, b_tokens_burnt, burnt_shares) =
            vault::withdraw(&e, &pool, &asset, &owner, assets);
        pool::withdraw(&e, &pool, &asset, &receiver, withdraw_amount);

        BlendVaultEvents::vault_withdraw(
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