//! This module contains the [`Wallets`] struct, which we use to manage and access the
//! Monero blockchain and wallets.
//!
//! Mostly we do two things:
//!  - wait for transactions to be confirmed
//!  - send money from one wallet to another.
pub use monero_sys::{Daemon, TxReceipt, WalletHandle as Wallet, WalletHandleListener};

use anyhow::{Context, Result};
use monero_address::Network;
use monero_daemon_rpc::MoneroDaemon;
use monero_oxide_wallet::transaction::{NotPruned, Transaction};
use monero_simple_request_rpc::SimpleRequestTransport;
use std::time::Duration;
use std::{path::PathBuf, sync::Arc};
use swap_core::monero::primitives::{Amount, BlockHeight, PrivateViewKey, TxHash};
use tokio::sync::RwLock;
use uuid::Uuid;
use zeroize::Zeroizing;

use crate::compat::tx_hash_to_bytes;
use crate::listener::{MoneroTauriHandle, TauriWalletListener};

/// Default poll interval for blockchain queries.
const POLL_INTERVAL: Duration = Duration::from_secs(10);

pub type TauriHandle = Arc<dyn MoneroTauriHandle>;

/// Entrance point to the Monero blockchain.
/// You can use this struct to open specific wallets and monitor the blockchain.
pub struct Wallets {
    /// The directory we store the wallets in.
    wallet_dir: PathBuf,
    /// The network we're on.
    network: Network,
    /// The monero node we connect to. The RPC client is connected lazily.
    daemon: Arc<RwLock<(Daemon, Option<MoneroDaemon<SimpleRequestTransport>>)>>,
    /// Keep the main wallet open and synced.
    main_wallet: Arc<Wallet>,
    /// Since Network::Regtest isn't a thing we have to use an extra flag.
    /// When we're in regtest mode, we need to unplug some safety nets to make the wallet work.
    regtest: bool,
    /// A handle we use to send status updates to the UI i.e. when
    /// waiting for a transaction to be confirmed.
    #[expect(dead_code)]
    tauri_handle: Option<TauriHandle>,
    /// Database for tracking wallet usage history.
    wallet_database: Option<Arc<monero_sys::Database>>,
}

impl Wallets {
    /// Create a new `Wallets` instance.
    /// Wallets will be opened on the specified network, connected to the specified daemon
    /// and stored in the specified directory.
    ///
    /// The main wallet will be kept alive and synced, other wallets are
    /// opened and closed on demand.
    pub async fn new(
        wallet_dir: PathBuf,
        main_wallet_name: String,
        daemon: Daemon,
        network: Network,
        regtest: bool,
        tauri_handle: Option<TauriHandle>,
        wallet_database: Option<Arc<monero_sys::Database>>,
    ) -> Result<Self> {
        let main_wallet = Wallet::open_or_create(
            wallet_dir.join(&main_wallet_name).display().to_string(),
            daemon.clone(),
            network,
            true,
        )
        .await
        .context("Failed to open main wallet")?;

        if regtest {
            main_wallet.unsafe_prepare_for_regtest().await;
        }

        let main_wallet = Arc::new(main_wallet);

        // We always register this listener
        // It does essential things like storing the wallet on certain events
        let handle_listener = WalletHandleListener::new(main_wallet.clone());
        main_wallet
            .call(move |wallet| {
                wallet.add_listener(Box::new(handle_listener));
            })
            .await?;

        // We only register the UI listener if we are running with Tauri
        if let Some(tauri_handle) = tauri_handle.clone() {
            let tauri_wallet_listener =
                TauriWalletListener::new(tauri_handle, main_wallet.clone()).await;

            main_wallet
                .call(move |wallet| {
                    wallet.add_listener(Box::new(tauri_wallet_listener));
                })
                .await
                .context("Failed to install tauri wallet listener")?;
        }

        let daemon = Arc::new(RwLock::new((daemon, None)));

        let wallets = Self {
            wallet_dir,
            network,
            daemon,
            main_wallet,
            regtest,
            tauri_handle,
            wallet_database,
        };

        // Record wallet access in database
        let wallet_path = wallets.main_wallet.path().await;
        let _ = wallets.record_wallet_access(&wallet_path?).await;

        Ok(wallets)
    }

    /// Create a new `Wallets` instance with an existing wallet as the main wallet.
    /// This is used when we want to use a user-selected wallet instead of creating a new one.
    pub async fn new_with_existing_wallet(
        wallet_dir: PathBuf,
        daemon: Daemon,
        network: Network,
        regtest: bool,
        tauri_handle: Option<TauriHandle>,
        existing_wallet: Wallet,
        wallet_database: Option<Arc<monero_sys::Database>>,
    ) -> Result<Self> {
        // TODO: This code is duplicated in [`Wallets::new`]. Unify it.
        if regtest {
            existing_wallet.unsafe_prepare_for_regtest().await;
        }

        let main_wallet = Arc::new(existing_wallet);

        let handle_listener = WalletHandleListener::new(main_wallet.clone());

        // We always register this listener.
        // It does essential things like storing the wallet on certain events
        main_wallet
            .call(move |wallet| {
                wallet.add_listener(Box::new(handle_listener));
            })
            .await?;

        // We only register the UI listener if we are running with Tauri
        if let Some(tauri_handle) = tauri_handle.clone() {
            let tauri_wallet_listener =
                TauriWalletListener::new(tauri_handle, main_wallet.clone()).await;

            main_wallet
                .call(move |wallet| {
                    wallet.add_listener(Box::new(tauri_wallet_listener));
                })
                .await?;
        }

        let daemon = Arc::new(RwLock::new((daemon, None)));

        let wallets = Self {
            wallet_dir,
            network,
            daemon,
            main_wallet,
            regtest,
            tauri_handle,
            wallet_database,
        };

        // Record wallet access in database
        let wallet_path = wallets.main_wallet.path().await;
        let _ = wallets.record_wallet_access(&wallet_path?).await;

        Ok(wallets)
    }

    pub async fn change_monero_node(&self, new_daemon: Daemon) -> Result<()> {
        // Reconnect lazily on next use.
        *self.daemon.write().await = (new_daemon.clone(), None);

        self.main_wallet
            .call(move |wallet| wallet.set_daemon(&new_daemon))
            .await??;

        Ok(())
    }
}

impl Wallets {
    /// Get the main wallet (specified when initializing the `Wallets` instance).
    pub async fn main_wallet(&self) -> Arc<Wallet> {
        self.main_wallet.clone()
    }

    /// Open the lock wallet of a specific swap from a given Monero TxLock ID.
    /// We can therefore only use this if we already know the txid of the lock transaction.
    /// It will skip the syncing and only import the given transaction.
    ///
    /// Used to redeem (Bob) or refund (Alice) the Monero.
    pub async fn swap_wallet_spendable(
        &self,
        swap_id: Uuid,
        spend_key: monero_oxide_ext::PrivateKey,
        view_key: PrivateViewKey,
        tx_lock_id: TxHash,
    ) -> Result<Arc<Wallet>> {
        // Derive wallet address from the keys
        let address = {
            let public_spend_key = monero_oxide_ext::PublicKey::from_private_key(&spend_key);
            let public_view_key = monero_oxide_ext::PublicKey::from_private_key(&view_key.into());

            monero_address::MoneroAddress::new(
                self.network,
                monero_address::AddressType::Legacy,
                public_spend_key.decompress(),
                public_view_key.decompress(),
            )
        };

        let wallet_path = swap_wallet_path(swap_id, &self.wallet_dir, true)
            .display()
            .to_string();

        // We get the current blockheight from the daemon
        let blockheight = self.direct_rpc_block_height().await?;

        let (daemon, _) = self.daemon.read().await.clone();

        let wallet = Wallet::open_or_create_from_keys(
            wallet_path.clone(),
            None,
            self.network,
            address,
            view_key.into(),
            spend_key,
            blockheight,
            // We don't sync the swap wallet, just import the transaction
            false,
            daemon,
        )
        .await
        .context(format!(
            "Failed to open or create wallet `{}` from the specified keys",
            wallet_path
        ))?;

        if self.regtest {
            wallet.unsafe_prepare_for_regtest().await;
        }

        tracing::debug!(
            %swap_id,
            "Opened temporary Monero wallet, loading lock transaction"
        );

        wallet
            .scan_transaction(tx_lock_id.0.clone())
            .await
            .context("Couldn't import Monero lock transaction")?;

        wallet.set_restore_height(blockheight).await?;

        // We synchronously refresh the wallet
        // This should be quick because we just set the restore height to the current blockheight.
        wallet.refresh_blocking().await?;

        // Now we start the refresh thread Why?
        // Because if the user later tries to spend the funds (after a new block is mined), the wallet will not be synchronized anymore
        // We start the refresh thread such that the wallet will keep up with the chain tip in the background.
        wallet.start_refresh_thread().await?;

        Ok(Arc::new(wallet))
    }
}

impl Wallets {
    /// Get the RPC client for direct daemon communication, connecting lazily on
    /// first use (and after a node change) so an unreachable node never blocks init.
    pub async fn rpc_client(&self) -> Result<MoneroDaemon<SimpleRequestTransport>> {
        if let Some(rpc_client) = &self.daemon.read().await.1 {
            return Ok(rpc_client.clone());
        }

        let mut guard = self.daemon.write().await;
        if let Some(rpc_client) = &guard.1 {
            return Ok(rpc_client.clone());
        }

        let rpc_client = SimpleRequestTransport::new(guard.0.to_url_string())
            .await
            .context("Failed to connect to Monero daemon")?;
        guard.1 = Some(rpc_client.clone());

        Ok(rpc_client)
    }

    /// Check that the daemon RPC is reachable, connecting if necessary.
    pub async fn rpc_health_check(&self) -> Result<()> {
        self.direct_rpc_block_height()
            .await
            .context("Monero daemon RPC health check failed")?;

        Ok(())
    }

    pub async fn is_transaction_present(&self, tx_hash: &TxHash) -> Result<bool> {
        use monero_wallet_ng::rpc::{ProvidesTransactionStatus, TransactionStatus};

        let rpc_client = self.rpc_client().await?;
        let tx_id = tx_hash_to_bytes(tx_hash)?;

        let status = rpc_client
            .transaction_status(tx_id)
            .await
            .context("Failed to query Monero transaction status")?;

        Ok(!matches!(status, TransactionStatus::Unknown))
    }

    /// Publish a transaction only if it is not already in the daemon's pool or blockchain.
    pub async fn ensure_transaction_published(&self, tx: &Transaction<NotPruned>) -> Result<()> {
        use monero_interface::PublishTransaction;

        if self.is_transaction_present(&TxHash::from_tx(tx)).await? {
            return Ok(());
        }

        self.rpc_client()
            .await?
            .publish_transaction(tx)
            .await
            .context("Failed to publish Monero transaction")
    }

    /// Returns true if any of `tx`'s inputs has already been spent by a transaction confirmed
    /// in the blockchain. Combined with `tx` itself not being present on-chain, this indicates
    /// a different transaction spent our inputs (a confirmed double spend).
    pub async fn has_input_confirmed_spent(&self, tx: &Transaction<NotPruned>) -> Result<bool> {
        use monero_wallet_ng::rpc::IsKeyImageSpent;

        let key_images = tx_key_images(tx);

        let statuses = self
            .rpc_client()
            .await?
            .is_key_image_spent(&key_images)
            .await
            .context("Failed to query key image spend status")?;

        Ok(any_confirmed_spent(&statuses))
    }

    /// Check for a conflicting input spend at the required canonical confirmation depth.
    pub async fn has_input_spent_with_confirmations(
        &self,
        tx: &Transaction<NotPruned>,
        restore_height: BlockHeight,
        required_confirmations: u64,
    ) -> Result<bool> {
        anyhow::ensure!(required_confirmations > 0, "Rebuild confirmations must be positive");
        if !self.has_input_confirmed_spent(tx).await? {
            return Ok(false);
        }

        monero_wallet_ng::double_spend::has_confirmed_conflict(
            &self.rpc_client().await?,
            tx.hash(),
            &tx_key_images(tx),
            usize::try_from(restore_height.height).context("Restore height exceeds usize")?,
            usize::try_from(required_confirmations).context("Rebuild confirmations exceed usize")?,
        )
        .await
        .context("Failed to establish confirmed Monero input conflict depth")
    }

    pub async fn direct_rpc_block_height(&self) -> Result<u64> {
        use monero_daemon_rpc::prelude::ProvidesBlockchainMeta;
        let rpc_client = self.rpc_client().await?;

        let height = rpc_client
            .latest_block_number()
            .await
            .context("Failed to get block height from daemon")?;

        Ok(height as u64)
    }

    pub async fn has_received_outputs(
        &self,
        public_spend_key: monero_oxide_ext::PublicKey,
        private_view_key: PrivateViewKey,
        start_height: BlockHeight,
        target_tip: Option<BlockHeight>,
        inner_retry: Option<backoff::ExponentialBackoff>,
    ) -> Result<bool> {
        let rpc_client = self.rpc_client().await?;
        let public_spend_key = public_spend_key.decompress();
        let private_view_key = Zeroizing::new(private_view_key.0.scalar);

        let start_height = usize::try_from(start_height.height)
            .context("Monero scan start height does not fit in usize")?;
        let target_tip = target_tip
            .map(|height| usize::try_from(height.height))
            .transpose()
            .context("Monero scan target tip does not fit in usize")?;

        monero_wallet_ng::empty::has_received_outputs(
            &rpc_client,
            public_spend_key,
            private_view_key,
            start_height,
            target_tip,
            inner_retry,
        )
        .await
        .context("Failed to check for received outputs")
    }

    /// Construct and sign a sweep transaction for the largest output of
    /// `lock_tx_hash` (viewable by the given view-pair) across a set of
    /// `destinations` split by ratio.
    ///
    /// This is the monero-wallet-ng-based replacement for the old monero-sys
    /// `sweep_multi_destination` flow: the caller hands over the keys of a
    /// single-use view-pair that only ever receives the lock transaction, and
    /// this method locates the block that contains `lock_tx_hash`, scans it,
    /// and sweeps its single output across the destinations.
    ///
    /// `destinations` must be non-empty and its ratios must sum to 1.
    pub async fn construct_sweep_to(
        &self,
        lock_tx_hash: &TxHash,
        spend_key: monero_oxide_ext::PrivateKey,
        view_key: PrivateViewKey,
        destinations: Vec<(monero_address::MoneroAddress, f64)>,
        inner_retry: Option<backoff::ExponentialBackoff>,
    ) -> Result<Transaction<NotPruned>> {
        let rpc_client = self.rpc_client().await?;
        let tx_id = tx_hash_to_bytes(lock_tx_hash)?;

        let spend_scalar = Zeroizing::new(spend_key.scalar);
        let view_scalar = Zeroizing::new(view_key.0.scalar);

        monero_wallet_ng::sweep::construct_sweep_tx_to(
            rpc_client,
            spend_scalar,
            view_scalar,
            tx_id,
            destinations,
            inner_retry,
        )
        .await
        .context("Failed to construct sweep transaction to destinations")
    }

    /// Convenience wrapper around [`Self::construct_sweep_to`] for the single-destination case.
    pub async fn construct_sweep_to_single(
        &self,
        lock_tx_hash: &TxHash,
        spend_key: monero_oxide_ext::PrivateKey,
        view_key: PrivateViewKey,
        destination: monero_address::MoneroAddress,
        inner_retry: Option<backoff::ExponentialBackoff>,
    ) -> Result<Transaction<NotPruned>> {
        let rpc_client = self.rpc_client().await?;
        let tx_id = tx_hash_to_bytes(lock_tx_hash)?;

        let spend_scalar = Zeroizing::new(spend_key.scalar);
        let view_scalar = Zeroizing::new(view_key.0.scalar);

        monero_wallet_ng::sweep::construct_sweep_tx_to_single(
            rpc_client,
            spend_scalar,
            view_scalar,
            tx_id,
            destination,
            inner_retry,
        )
        .await
        .context("Failed to construct sweep transaction to destination")
    }

    /// Construct a transaction carrying `data` in its tx_extra: it spends the
    /// output of `funding_tx_hash`, pays a single piconero to `destination`
    /// and the entire remainder as fee.
    pub async fn construct_data_tx(
        &self,
        funding_tx_hash: &TxHash,
        spend_key: monero_oxide_ext::PrivateKey,
        view_key: PrivateViewKey,
        destination: monero_address::MoneroAddress,
        data: Vec<Vec<u8>>,
        inner_retry: Option<backoff::ExponentialBackoff>,
    ) -> Result<Transaction<NotPruned>> {
        let rpc_client = self.rpc_client().await?;
        let tx_id = tx_hash_to_bytes(funding_tx_hash)?;

        let spend_scalar = Zeroizing::new(spend_key.scalar);
        let view_scalar = Zeroizing::new(view_key.0.scalar);

        monero_wallet_ng::sweep::construct_data_tx(
            rpc_client,
            spend_scalar,
            view_scalar,
            tx_id,
            destination,
            data,
            inner_retry,
        )
        .await
        .context("Failed to construct data transaction")
    }

    pub async fn construct_multi_destination_tx(
        &self,
        destinations: &[(monero_address::MoneroAddress, monero_oxide_ext::Amount)],
    ) -> Result<(Transaction<NotPruned>, TxReceipt)> {
        let (receipt, tx_hex) = self
            .main_wallet()
            .await
            .construct_multi_destination_tx(destinations)
            .await
            .context("Failed to construct Monero transaction")?;

        let tx = monero_wallet_ng::util::transaction_from_hex(&tx_hex)
            .context("Failed to parse constructed Monero transaction")?;

        let tx_hash = hex::encode(tx.hash());
        anyhow::ensure!(
            tx_hash == receipt.txid,
            "Parsed Monero transaction hash {} does not match wallet-reported txid {}",
            tx_hash,
            receipt.txid
        );

        Ok((tx, receipt))
    }

    /// Verify a transfer using the new monero-wallet-ng implementation.
    ///
    /// This verifies that a transaction sends the expected amount to the given view pair
    /// by fetching the transaction from the RPC and scanning it.
    pub async fn verify_transfer(
        &self,
        tx_hash: &TxHash,
        public_spend_key: monero_oxide_ext::PublicKey,
        private_view_key: PrivateViewKey,
        expected_amount: Amount,
    ) -> Result<bool> {
        let rpc_client = self.rpc_client().await?;

        let tx_id = tx_hash_to_bytes(tx_hash)?;
        let public_spend_key = public_spend_key.decompress();
        let private_view_key = Zeroizing::new(private_view_key.0.scalar);
        let expected_amount = expected_amount.as_pico();

        let result = monero_wallet_ng::verify::verify_transfer(
            &rpc_client,
            tx_id,
            public_spend_key,
            private_view_key,
            expected_amount,
        )
        .await
        .context("Failed to verify transfer")?;

        Ok(result)
    }

    /// The amount of the largest output the given view pair receives in
    /// `tx_hash`, or `None` if it receives no outputs. This mirrors what a
    /// sweep of the transaction can spend.
    pub async fn largest_received_utxo(
        &self,
        tx_hash: &TxHash,
        public_spend_key: monero_oxide_ext::PublicKey,
        private_view_key: PrivateViewKey,
    ) -> Result<Option<Amount>> {
        let rpc_client = self.rpc_client().await?;

        let tx_id = tx_hash_to_bytes(tx_hash)?;
        let public_spend_key = public_spend_key.decompress();
        let private_view_key = Zeroizing::new(private_view_key.0.scalar);

        let amount = monero_wallet_ng::verify::largest_received_utxo(
            &rpc_client,
            tx_id,
            public_spend_key,
            private_view_key,
        )
        .await
        .context("Failed to scan transaction for received amount")?;

        Ok(amount.map(Amount::from_pico))
    }

    /// Wait until a transfer is verified and confirmed using monero-wallet-ng.
    ///
    /// This first verifies that the transaction sends the expected amount to the given view pair,
    /// then subscribes to confirmations and waits until the target confirmations are reached.
    ///
    /// You can pass a listener function that will be called with
    /// the current number of confirmations every time the status changes.
    pub async fn wait_until_confirmed(
        &self,
        tx_hash: &TxHash,
        confirmation_target: u64,
        listener: Option<impl Fn((TxHash, u64, u64)) + Send + 'static>,
    ) -> Result<()> {
        use monero_wallet_ng::confirmations;

        let rpc_client = self.rpc_client().await?;
        let tx_id = tx_hash_to_bytes(tx_hash)?;
        let subscription = confirmations::subscribe(rpc_client, tx_id, POLL_INTERVAL);

        // Wait until target confirmations, emitting events on each status change
        subscription
            .wait_until(|status| {
                let current = status.confirmations();

                if let Some(ref listener) = listener {
                    listener((tx_hash.clone(), current, confirmation_target));
                }

                tracing::debug!(
                    confirmations = current,
                    target = confirmation_target,
                    "Monero transaction confirmation status update"
                );

                status.has_confirmations(confirmation_target)
            })
            .await
            .context("Subscription closed before reaching target confirmations")?;

        tracing::debug!(
            tx_hash = %tx_hash.0,
            "Monero transaction fully confirmed"
        );

        Ok(())
    }

    /// Wait for an incoming transfer using the new monero-wallet-ng scanner.
    ///
    /// This scans the blockchain from `restore_height` looking for an output
    /// with the expected amount sent to the given view pair. Returns the
    /// transaction hash when found.
    pub async fn wait_for_incoming_transfer(
        &self,
        public_spend_key: monero_oxide_ext::PublicKey,
        private_view_key: PrivateViewKey,
        expected_amount: Amount,
        restore_height: BlockHeight,
    ) -> Result<TxHash> {
        use monero_wallet_ng::scanner;

        let rpc_client = self.rpc_client().await?;

        let public_spend_key = public_spend_key.decompress();
        let private_view_key = Zeroizing::new(private_view_key.0.scalar);

        let expected_amount = expected_amount.as_pico();
        let restore_height = restore_height.height as usize;

        let mut subscription = scanner::naive_scanner(
            rpc_client,
            public_spend_key,
            private_view_key,
            restore_height,
            POLL_INTERVAL,
        )
        .context("Failed to create scanner")?;

        let output = subscription
            .wait_until(|output| output.commitment().amount == expected_amount)
            .await
            .context("Scanner subscription closed before finding output")?;

        let tx_hash = hex::encode(output.transaction());

        tracing::debug!(
            %tx_hash,
            amount = expected_amount,
            "Found incoming transfer with expected amount"
        );

        Ok(TxHash(tx_hash))
    }

    /// Scan the wallet of the given view pair from `restore_height` until an
    /// output carries a Hermes message that `extract` accepts, returning the
    /// extracted value.
    ///
    /// Outputs that do not contain a Hermes message at all are skipped
    /// silently. Outputs that do contain a Hermes message but are rejected by
    /// `extract` are skipped too, but the rejection reason is debug-logged so we
    /// can tell why an on-chain Hermes transaction was ignored. This keeps the
    /// scanner moving past invalid messages instead of getting stuck on them.
    pub async fn wait_for_hermes_message<T>(
        &self,
        public_spend_key: monero_oxide_ext::PublicKey,
        private_view_key: PrivateViewKey,
        restore_height: BlockHeight,
        extract: impl Fn(&monero_wallet_ng::hermes::HermesMessage) -> Result<T>,
    ) -> Result<T> {
        use monero_wallet_ng::hermes::HermesMessage;
        use monero_wallet_ng::scanner;

        let rpc_client = self.rpc_client().await?;

        let public_spend_key = public_spend_key.decompress();
        let view_scalar = Zeroizing::new(private_view_key.0.scalar);

        let mut subscription = scanner::naive_scanner(
            rpc_client,
            public_spend_key,
            view_scalar.clone(),
            restore_height.height as usize,
            POLL_INTERVAL,
        )
        .context("Failed to create scanner")?;

        let mut extracted: Option<T> = None;

        subscription
            .wait_until(|output| {
                // Outputs that aren't Hermes messages are the common case while
                // scanning; skip them without logging to avoid spamming.
                let Ok(message) = HermesMessage::from_wallet_output(output, view_scalar.clone())
                else {
                    return false;
                };

                match extract(&message) {
                    Ok(value) => {
                        tracing::debug!(
                            tx_hash = %hex::encode(output.transaction()),
                            "Found accepted Hermes message"
                        );
                        extracted = Some(value);
                        true
                    }
                    Err(error) => {
                        tracing::debug!(
                            tx_hash = %hex::encode(output.transaction()),
                            error = ?error,
                            "Ignoring Hermes message rejected by filter"
                        );
                        false
                    }
                }
            })
            .await
            .context("Scanner subscription closed before finding an accepted Hermes message")?;

        Ok(extracted.expect("output was accepted because extract returned Ok"))
    }
}

impl Wallets {
    /// Get the last 5 recently used wallets
    pub async fn get_recent_wallets(&self) -> Result<Vec<String>> {
        if let Some(db) = &self.wallet_database {
            let recent_wallets = db.get_recent_wallets(5).await?;
            Ok(recent_wallets.into_iter().map(|w| w.wallet_path).collect())
        } else {
            Ok(vec![])
        }
    }

    /// Record that a wallet was accessed
    pub async fn record_wallet_access(&self, wallet_path: &str) -> Result<()> {
        if let Some(db) = &self.wallet_database {
            db.record_wallet_access(wallet_path).await?;
        }
        Ok(())
    }
}

/// Pass this to [`Wallet::wait_until_confirmed`] or [`Wallet::wait_until_synced`]
/// to not receive any confirmation callbacks.
pub fn no_listener<T>() -> Option<impl Fn(T) + Send + 'static> {
    Some(|_| {})
}

fn swap_wallet_path(swap_id: Uuid, wallet_dir: &PathBuf, spendable: bool) -> PathBuf {
    let suffix = if spendable { "spendable" } else { "view_only" };
    let name = format!("swap_{}_{}", &swap_id.to_string(), suffix);

    wallet_dir.join(name)
}

/// Extracts the key images of all `ToKey` inputs of `tx`.
fn tx_key_images(tx: &Transaction<NotPruned>) -> Vec<[u8; 32]> {
    use monero_oxide_wallet::transaction::Input;

    tx.prefix()
        .inputs
        .iter()
        .filter_map(|input| match input {
            Input::ToKey { key_image, .. } => Some(key_image.to_bytes()),
            Input::Gen(_) => None,
        })
        .collect()
}

/// Returns true if any key image was spent by a transaction confirmed in the
/// blockchain. A spend that only exists in the mempool does not count: the
/// conflicting transaction may still be displaced by our own transaction.
fn any_confirmed_spent(statuses: &[monero_wallet_ng::rpc::KeyImageSpentStatus]) -> bool {
    use monero_wallet_ng::rpc::KeyImageSpentStatus;

    statuses
        .iter()
        .any(|status| matches!(status, KeyImageSpentStatus::SpentInBlockchain))
}

#[cfg(test)]
mod tests {
    use super::any_confirmed_spent;
    use monero_wallet_ng::rpc::KeyImageSpentStatus;

    #[test]
    fn no_statuses_is_not_a_confirmed_spend() {
        assert!(!any_confirmed_spent(&[]));
    }

    #[test]
    fn unspent_is_not_a_confirmed_spend() {
        assert!(!any_confirmed_spent(&[KeyImageSpentStatus::Unspent]));
    }

    /// Crucial anti-manipulation property: a key image spent only by a mempool
    /// transaction must never be treated as a confirmed double spend.
    #[test]
    fn pool_spend_is_not_a_confirmed_spend() {
        assert!(!any_confirmed_spent(&[KeyImageSpentStatus::SpentInPool]));
        assert!(!any_confirmed_spent(&[
            KeyImageSpentStatus::Unspent,
            KeyImageSpentStatus::SpentInPool,
        ]));
    }

    #[test]
    fn blockchain_spend_is_a_confirmed_spend() {
        assert!(any_confirmed_spent(&[
            KeyImageSpentStatus::SpentInBlockchain
        ]));
        assert!(any_confirmed_spent(&[
            KeyImageSpentStatus::Unspent,
            KeyImageSpentStatus::SpentInPool,
            KeyImageSpentStatus::SpentInBlockchain,
        ]));
    }
}
