# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

- GUI: Support outbound connections to makers through libp2p circuit relays.
- ASB: Fix concurrent swaps selecting the same Monero output when building their lock transactions,
  which turned the losing swap's lock transaction into a permanent double-spend
  and left the taker's swap stuck until it refunded.
  A concurrent swap the maker can no longer fund is now refunded early,
  instead of building a doomed lock transaction that wedges the swap.

## [4.14.0] - 2026-08-22

- ASB: The `get-swaps` RPC response now includes the Bitcoin redeem address for each swap.

## [4.13.3] - 2026-08-05

- GUI: Fix a number of small ui issues / inconsistencies

## [4.13.2] - 2026-07-23

- GUI + ASB: Remove nine unusable Electrum defaults and add healthy Mullvad,
  Cake Wallet, Blockstream, DIYnodes, and ACINQ mainnet servers.

## [4.13.1] - 2026-07-23

## [4.13.0] - 2026-07-23

- ASB: Fix a bug in the algorithm used to the select the Bitcoin output used for the swap.
- ASB+GUI: Fix a bug in the cross curve equality proof cryptography.

## [4.12.0] - 2026-07-18

- GUI: The wallet setup dialog is now a multi-step wizard. Creating a wallet lets you pick a name and location and requires backing up the seed phrase before continuing, and opening a wallet file supports drag & drop. Newly created wallets are named by the user instead of a Unix timestamp.

## [4.11.5] - 2026-07-07

- ASB: The Hermes protocol is now enabled by default (`hermes_enabled` defaults to `true`), and the default `hermes_min_swap_amount` was lowered from `0.01` to `0.001` BTC (~50 USD at a reference price of 50,000 USD/BTC).

## [4.11.4] - 2026-06-30

## [4.11.3] - 2026-06-24

## [4.11.2] - 2026-06-23

## [4.11.0] - 2026-06-23

- PROTOCOL: A new protcol called "Hermes" has been implemented. It allows taker and maker to communicate through the Monero blockchain. Monero transactions are used for passing messages. This is used for the taker to transmit the encrypted signature to the maker without requiring a network connection. This means a swap can now succeed without any p2p connection to the other party after the intitial swap setup.
- ASB: Added new `[maker]` config options for the Hermes protocol:
  - `hermes_enabled`: whether to fund the on-chain Hermes encrypted-signature channel at all (default: `false`).
  - `hermes_funding_amount_piconero`: amount of Monero (in piconero) attached to the Monero lock transaction to fund the Hermes transaction (default: `100000000`, i.e. 0.0001 XMR).
  - `hermes_min_swap_amount`: minimum swap size (in BTC) below which the Hermes amount is not funded (default: `0.01`).

## [4.10.2] - 2026-06-22

## [4.10.1] - 2026-06-20

## [4.10.0] - 2026-06-19

- GUI + ASB: Speed up Bitcoin wallet calls by waiting for less than all Electrum servers to respond.

## [4.9.6] - 2026-06-17

- GUI: Allow user to copy raw hex of the Monero redeem transaction to broadcast externally.
- GUI: Continuously republish the Monero redeem transaction while we are waiting for it to confirm.

## [4.9.5] - 2026-06-16

## [4.9.4] - 2026-06-16

## [4.9.3] - 2026-06-15

## [4.9.2] - 2026-06-13

- Added a set of new community maintained rendezvous points. Please configure your makers to register at them:`/dns4/discovery.eigenwallet.org/tcp/443/wss/p2p/12D3KooWGRvf7qVQDrNR5nfYD6rKrbgeTi9x8RrbdxbmsPvxL4mw`,
  `/onion3/3xl2zfur4tpebogsrgn3l7l2illzkhwi3755jplmycmn4q77nxsrl6qd:8888/p2p/12D3KooWGRvf7qVQDrNR5nfYD6rKrbgeTi9x8RrbdxbmsPvxL4mw`,
  `/dns4/rendezvous.atomicworld.fun/tcp/443/wss/p2p/12D3KooWMc39w7bZz4RLmJKuUiK9YkbKoEHACZWcL71XNns5dPuD`,
  `/onion3/m2iuwp3fvdlqtlqqaz3egrzjl5uehmdhjgmzhznvjoudljl2xzjaomyd:8890/p2p/12D3KooWMc39w7bZz4RLmJKuUiK9YkbKoEHACZWcL71XNns5dPuD`,
  `/dns4/dht.stealthswap.ninja/tcp/443/wss/p2p/12D3KooWGjcxdpsEWspGGwkQJ9BRJQjtBQFsLk36zJxrXSBPQWov`,
  `/onion3/m6rboz5lv4wxldgybgox4pr4s6xci3h2exi5nogxaox762xji2gokuad:8891/p2p/12D3KooWGjcxdpsEWspGGwkQJ9BRJQjtBQFsLk36zJxrXSBPQWov`,
  `/dns4/discovery2.eigenwallet.org/tcp/443/wss/p2p/12D3KooWA6cnqJpVnreBVnoro8midDL9Lpzmg8oJPoAGi7YYaamE`,
  `/onion3/av2jauifny7dgpvzhsnhra3cwivf6ofaefxvwhhuh5y7hsolabehhaad:8888/p2p/12D3KooWA6cnqJpVnreBVnoro8midDL9Lpzmg8oJPoAGi7YYaamE`,

## [4.9.1] - 2026-06-12

## [4.9.0] - 2026-06-10

- ASB+CONTROLLER: The JSON-RPC server now requires authentication. The ASB verifies a password against a hashed keyfile (`--rpc-auth-file`), and `asb-controller` prompts for the password on startup. Generate the keyfile with `orchestrator gen-rpc-auth`. Clients authenticate by sending the password with every request in an `Authorization: Bearer <password>` header.
- ASB+GUI: Skip publishing the Monero redeem/refund transaction if it is already present on chain (e.g. after a restart)

## [4.8.4] - 2026-06-09

## [4.8.3] - 2026-06-08

- Reliably retry Monero redeem step

## [4.8.2] - 2026-06-06

## [4.8.1] - 2026-06-05

## [4.8.0] - 2026-06-05

## [4.7.11] - 2026-06-05

- GUI: Fix flickering of offers
- Remove three dead rendezvous points

## [4.7.10] - 2026-06-02

## [4.7.9] - 2026-05-29

- ASB: The ASB will refuse to cooperate with cooperative XMR redeem requests if the swap in question was deemed malicious. A swap is considered malicious if for any reason the Bitcoin amount the ASB receives is less than 75% of what was sent into the swap originally.

## [4.7.8] - 2026-05-28

## [4.6.7] - 2026-05-27

## [4.6.6] - 2026-05-27

- ASB+GUI: Both parties will now reject swaps where the other party's proposed fee is too far off from the own fee estimate.
- ASB+CONTROLLER: `get-swaps` now includes the `btc_punish_txid` per swap: the Bitcoin punish transaction id. This is deterministic from the swap's locked state, so it is set even before the punish transaction is published.

## [4.6.5] - 2026-05-26

## [4.6.4] - 2026-05-21

## [4.6.3] - 2026-05-21

- CLI: Outbound Tor dials are now concurrency-limited and spaced out, with a separate higher-throughput lane for high-priority peers, so bursts of dials no longer overwhelm the embedded Tor client.
- ASB+CONTROLLER: New `get-external-bitcoin-redeem-address` command returns the external Bitcoin redeem address currently used by the ASB (or `null` if the internal wallet is used).

## [4.6.1] - 2026-05-15

- ASB+CONTROLLER: New `set-external-bitcoin-redeem-address` command with parameter `address` (string) allows you to udpate the external bitcoin redeem address at runtime.
  The change is persisted to the config file. Go back to using the internal Bitcoin wallet by using `clear-external-bitcoin-redeem-address`.

## [4.6.0] - 2026-05-13

- GUI: Exolix has become a community supporter. They support development and provide a reliable market maker. They are visually highlighted in the UI.

## [4.5.5] - 2026-05-13

## [4.5.4] - 2026-05-12

## [4.5.3] - 2026-05-08

- ASB: New `maker.btc_redeem_fee_multiplier` config option (default `1.0`) that scales the estimated BTC redeem fee. Setting it higher (e.g. `2.0`) acts as a safety margin so the redeem still confirms when fee estimation undershoots actual mempool conditions.

## [4.5.2] - 2026-05-07

- ASB+CONTROLLER: `get-swaps` now includes the `btc_redeem_txid` per swap: the Bitcoin redeem transaction id. This is deterministic from the swap's locked state, so it is set even before the redeem transaction is published.

## [4.5.1] - 2026-05-05

- ASB+CONTROLLER: `get-swaps` now accepts optional `limit` and `offset` parameters which can be used for pagination.
  If not present, no pagination will be done.

## [4.5.0] - 2026-04-27

- ASB+CONTROLLER: `get-swaps` now includes the `btc_redeem_fee` per swap: the fee Alice paid (or will pay) for the Bitcoin redeem transaction.
- GUI+ASB: New intermediate states around Monero redeem/refund. The signed transaction is now built and published as separate steps: `XmrRedeemConstructed` and `XmrRedeemPublished` (Bob/GUI), and `XmrRefundTxConstructed` and `XmrRefundTxPublished` (Alice/ASB). The GUI surfaces both phases — "constructing", then "publishing", then waiting for the first confirmation with the redeem txid shown.

## [4.4.1] - 2026-04-15

- ASB+CONTROLLER: New `get-current-quote` command returns the quote the ASB is currently serving to peers (price per XMR, min and max quantity). Reuses the in-flight quote cache so repeated calls don't trigger extra work.
- ASB: Added Exolix as an optional XMR/BTC price source. Set `maker.price_ticker_source_exolix_api_key` in the config to enable; the polled rate is averaged alongside Kraken, Bitfinex, and KuCoin. Poll interval is configurable via `maker.price_ticker_rest_poll_interval_exolix_secs` (default: `10`).
- ASB: Each price source can now be individually disabled via `maker.price_ticker_source_kraken_enabled`, `maker.price_ticker_source_bitfinex_enabled`, and `maker.price_ticker_source_kucoin_enabled` (all default `true`). At least one source must remain enabled.
- ASB: How long a polled exchange-rate sample is considered valid is now configurable via `maker.price_ticker_validity_duration_secs` (default: `600`, i.e. 10 minutes).
- ASB: Fix an issue where the Bitfinex price was broken due to a change in the Bitfinex API.

## [4.4.0] - 2026-04-13

- ASB: Wormhole eligibility now only considers swaps whose most recent state update falls within a configurable freshness window. This is controlled by the new `tor.wormhole_swap_freshness_hours` config option (default: `168`, i.e. 7 days). Inactive peers no longer keep their wormhole indefinitely.
- GUI: Allow sorting of maker offers on the swap page by largest max amount (default), smallest min amount, or cheapest price, via a subtle sort button above the offer list.
- Improve Tor connectivity

## [4.3.1] - 2026-04-11

## [4.3.0] - 2026-04-09

- A new feature called "wormholes" allows makers to automatically give out dedidcated onion services to takers which have committed funds to at least one swap. Takers can then connect to the maker irrespective of any potential DOS attack against the makers public onion service address. Wormholes are enabled by default. To disable them, set `tor.wormhole_enabled = false` in the config file. You can tweak `wormhole_max_concurrent_rend_requests` to control the maximum number of concurrent rendezvous requests per wormhole (default: 3). A higher value means each "wormhole" can handle more concurrent connections but also means they become more susceptible to DOS attacks.

## [4.2.4] - 2026-04-01

## [4.2.3] - 2026-03-31

- GUI: Fix an issue where we could get stuck in the "BtcCancelled" state if the swap was punished.
- ASB: Fix issue where a database migration would cause the ASB to fail to start.

## [4.2.2] - 2026-03-31

- ASB+CONTROLLER: Fix a bug where `get-swaps` would show "No swaps found" despite there being swaps.
- ASB: Add limits to prevent denial-of-service via resource exhaustion.

## [4.1.1] - 2026-03-26

- ASB: Optimize how we respond to network request

## [4.1.0] - 2026-03-25

- GUI: Fix an issue where the application would crash when the internal Tor client tried to execute a Proof-of-Work challenge due to a missing entitlement permission. ([#915](https://github.com/eigenwallet/core/issues/915))

## [4.0.5] - 2026-03-24

- Try to fix an issue where the response from the Monero daemon would be too large to parse when fetching the status of a transaction ([#885](https://github.com/eigenwallet/core/issues/885)).

## [4.0.4] - 2026-03-20

- ASB: Tweak some networking configurations to make DOS harder.

## [4.0.3] - 2026-03-18

- ASB: Attempt fix of `get-swaps` not showing swaps.
- ASB: Fix `btc_amnesty_amount missing` bug which prevented swaps from before 4.0.0 from running.

## [4.0.2] - 2026-03-18

## [4.0.1] - 2026-03-17

- ASB: Fix bug where old swaps would not deserialize if they we refunded.

## [4.0.0] - 2026-03-16

- Protocol: Reduce cancel timelock to 24 blocks (4 hours).
  Swaps can now be refunded after 4 hours, instead of the previous 12 hours.
  This also means the refund window ends after `4 + 24 = 28` hours instead of the previous `12 + 24 = 36` hours.
  The punish timelock, which determines the length of the refund window, remains at 72 blocks (24 hours).

- Protocol: Add possibility for maker to require an "anti-spam deposit".
  The deposit is a part of the Bitcoin refund which the maker may withhold during a 30 minute timeframe
  The deposit can still be released after the fact, by granting mercy.
  Both parties will refuse an anti-spam deposit that makes up more than 20% of the swap's Bitcoin.

- GUI: Add a reputation chip for each offer showing the number of successful, refunded and bad swaps.
  Bad swaps are swaps during which the maker has behaved in a way that hurt the taker, like: punishing the taker or withholding the anti-spam deposit.
  This can be remedied by cooperative redeem / granting mercy respectively.

- GUI: Add a chip for each offer showing the guaranteed refund percentage and the required anti-spam deposit percentage.
  Colorcoded on a gradient with 100% refund being green and 90% being yellow.

- ASB + CONTROLLER: Add `set-withhold-deposit <swap-id> <true / false>` and `grant-mercy <swap-id>` commands.
  `set-withhold-deposit` must be called before the maker reaches `XmrRefunded` to be effective.
  `TxWithhold` will be published after the asb refunded the Monero.
  `grant-mercy` can only be called once the maker has entered `BtcWithheld`.

- ASB: New config option `maker.refund_policy.anti_spam_deposit_ratio`.
  It sets the ratio of the Bitcoin lock that will go into the deposit in case of a refund.
  Set it to `0.02` for an anti-spam deposit of 2%.
  Set it to `0.00` to offer full refunds (0% anti-spam deposit).
  Defaults to `0.00`.
  ```toml
  [maker.refund_policy]
  anti_spam_deposit_ratio = 0.02
  ```

## [3.7.0] - 2026-03-05

- ASB + CONTROLLER: Add `withdraw-btc` and `refresh-bitcoin-wallet` JSON-RPC commands. `withdraw-btc` allows withdrawing BTC from the internal Bitcoin wallet to a specified address. The amount parameter on the wire protocol is in satoshis (`Option<u64>`, `null` to sweep the entire balance). The `asb-controller` accepts human-friendly amounts (e.g. `0.1 BTC`, `10000 sat`). `refresh-bitcoin-wallet` syncs the internal Bitcoin wallet with the blockchain.

## [3.6.7] - 2026-01-22

## [3.6.6] - 2026-01-21

## [3.6.4] - 2026-01-05

- GUI: Bump GNOME Flatpak runtime to 48
- GUI: Add support for subaddress in the Monero wallet (thanks to @rafael-xmr !)

## [3.6.3] - 2025-12-23

## [3.6.2] - 2025-12-22

## [3.6.1] - 2025-12-15

- GUI (Taker): Previously takers had to receive the Monero transfer proof from the maker over the network. This required interactivity meaning that if the peer-to-peer connection was lost, the taker might not detect that the Monero were locked. This would cause unnecessary refunds of swaps that could have otherwise succeded. The taker will now scan the view-only Monero wallet in the background (while concurrently waiting for the transfer proof over the network). The taker will detect if the Monero are locked even if peer-to-peer connection to the other party is lost. This significantly improves the reliability of swaps.
- GUI: Fix an issue where it'd take a while for swaps to show up after a restart.

## [3.5.2] - 2025-12-08

## [3.5.1] - 2025-12-07

## [3.5.0] - 2025-12-04

- ASB: Now also uses Bitfinex and KuCoin as a source of truth for XMR/BTC exchange rates. It is used to calculate the base price for quotes upon which the markup is then added. If one or two of the three exchanges is unreachable, it will fallback to the arithmetic average of the rate of the remaining available exchanges.
- ASB: To protect against market manipulation if any of three rates from the exchanges differs from the arithmetic average of the others by 10%, the maker will refuse to make new quotes and refuse to initiate new swaps.
- ASB: `price_ticker_ws_url` config item renamed to `price_ticker_ws_url_kraken`. Two new config items `price_ticker_ws_url_bitfinex` (defaults to `wss://api-pub.bitfinex.com/ws/2`) and `price_ticker_rest_url_kucoin` (defaults to `https://api.kucoin.com/api/v1/bullet-public`)
- GUI + ASB: Potentially fix an issue where we would not properly connect to peers which are only reachable over a hidden service if the client was temporarily cut off from the internet.
- GUI: Significantly improve the rendering performance.
- GUI: Added a button to refresh the peer-to-peer connections (redials disconnected peers, re-fetches quotes, refreshes peer-discovery, ...)

## [3.4.2] - 2025-12-01

## [3.4.1] - 2025-12-01

- GUI: Display detailed information about which peers we are connected to.
- GUI: Change the unique avatars of makers to be distinctly different from each other to allow for recognizability.
- GUI + SWAP + ASB: Massivly improved the reliability of all parts of the P2P networking stack.
- ASB: Fix bugs where we would not properly reconnect to rendezvous servers which could negatively impact peer discovery
- CLI: Removed the `list-sellers` and `buy-xmr` CLI commands. They were cumbersome to use and confusing. They will be re-added once the CLI is more flexible to allow for interactivity.

## [3.3.4] - 2025-11-14

## [3.3.8] - 2025-11-30

- ASB + CONTROLLER: Add the `peer-id` command to the controller shell which can be used to obtain the Peer ID of your ASB instance.
- GUI: Fix an issue where the Monero wallet could become unresponsive

## [3.3.7] - 2025-11-19

- ORCHESTRATOR + ASB: Support for tunneling both the Bitcoin and Monero node over Tor has been added (thanks to @nabijaczleweli)

## [3.3.6] - 2025-11-17

- GUI: Fix an issue where the user would have to keep resuming if we failed to check the status of a Bitcoin timelock before we waited for the Monero lock transaction to be confirmed.

## [3.3.5] - 2025-11-15

- GUI: Allow changing the password of a Monero wallet (thanks to @nabijaczleweli)
- GUI: Fix an issue where the list of Electrum servers would display all servers as being unreachable (thanks to @nabijaczleweli)
- ASB: Fix an issue where we would not properly reconnect to rendezvous servers which could negatively impact peer discovery

## [3.3.3] - 2025-11-13

## [3.3.2] - 2025-11-13

- GUI + SWAP: Fix an issue where we would fail to connect to peers if we failed on the initial attempt because previous addresses were not cached properly.

## [3.3.1] - 2025-11-11

- GUI: Fix the Flatpak images to ensure they are kept up to date and the correct version is displayed. Also fixes an issue where a blank screen would sometimes be rendered. Big thanks to [nabijaczleweli](https://github.com/nabijaczleweli) for spending their time on this! Consider sending a Monero tip to the donation address pinned on their [Github profile](https://github.com/nabijaczleweli).

## [3.3.0] - 2025-11-10

- GUI + SWAP: Retry sending the encrypted signature more aggressively. This might help with an issue where we would be stuck on the "Sending encrypted signature" screen for a longer time than necessary.
- GUI + SWAP + ASB: Require 10 Monero confirmations again

## [3.2.11] - 2025-11-09

- GUI + SWAP: Assume double spend safety of Monero transactions after 6 confirmations. This means we are assuming that there won't be any re-orgs deeper than 5 blocks. We believe this is a safe assumption given that there were almost no orphaned blocks over the last two weeks. Qubic (which was behind the re-orgs) has mined less than 1% of the last 1000 blocks.
- GUI: Remove the following default Electrum servers: `tcp://electrum.blockstream.info:50001`, `tcp://electrum.coinucopia.io:50001`, `tcp://se-mma-crypto-payments-001.mullvad.net:50001`, `tcp://electrum2.bluewallet.io:50777` due to them being unreliable. Add the following new default Electrum servers: `tcp://electrum1.bluewallet.io:50001`, `tcp://electrum2.bluewallet.io:50001`, `tcp://electrum3.bluewallet.io:50001`, `ssl://btc-electrum.cakewallet.com:50002`, `tcp://bitcoin.aranguren.org:50001`.

## [3.2.10] - 2025-11-08

- GUI + SWAP + ASB: Reduce the confirmations required to spend a Monero transaction from 22 to 15. We believe the risks of re-orgs is low again and this is safe to do. This may increase the chances of swap being successful and will reduce the time a swap takes.
- GUI: Fix an issue where we a manual resume of a swap would be necessary if we failed to fetch certain Bitcoin transactions due to network issues.
-

## [3.2.9] - 2025-11-05

- GUI: Fix an issue where an error in the UI runtime would cause a white screen to be displayed and nothing would be rendered.
- GUI(Linux): Fix an issue where the GUI would display a white screen on some systems (among others Fedora 43)

## [3.2.8] - 2025-11-02

- ASB + CONTROLLER: Add a `registration-status` command to the controller shell. You can use it to get the registration status of the ASB at the configured rendezvous points.
- ASB + GUI + CLI + SWAP: Split high-verbosity tracing into separate hourly-rotating JSON log files per subsystem to reduce noise and aid debugging: `tracing*.log` (core things), `tracing-tor*.log` (purely tor related), `tracing-libp2p*.log` (low level networking), `tracing-monero-wallet*.log` (low level Monero wallet related). `swap-all.log` remains for non-verbose logs.
- ASB: Fix an issue where we would not redeem the Bitcoin and force a refund even though it was still possible to do so.
- GUI: Potentially fix issue here swaps would not be displayed

## [3.2.7] - 2025-10-28

## [3.2.6] - 2025-10-27

## [3.2.5] - 2025-10-26

- ASB: Fixed an issue where we would be forced to refund a swap if Bobs acknowledgement of the transfer proof did not reach us.
- RENDEZVOUS-NODE: Fix an issue where the `--data-dir` argument was not accepted

## [3.2.4] - 2025-10-26

## [3.2.3] - 2025-10-26

- RENDEZVOUS-NODE: Fix a spelling mistake in the Dockerfile

## [3.2.2] - 2025-10-25

- RENDEZVOUS-NODE: Now takes a `--data-dir` argument and has been renamed to "rendezvous-node" (previously "rendezvous-server")
- RENDEZVOUS-NODE: Rendezvous servers now register themselves at bootstrap rendezvous points to make them discoverable.
- ORCHESTRATOR: The orchestrator will now also add a `rendezvous-node` service to the `docker-compose.yml` file. Rendezvous nodes help with peer discovery in the network.
- ASB + GUI + CLI: Upgrade arti-client to 1.6.0

## [3.2.1] - 2025-10-21

- ASB + GUI + CLI: Fix an issue where the internal Tor client would fail to choose guards. This would prevent all Tor traffic from working. We temporarily fix this by forcing new guards to be chosen on every startup. This will be reverted once the issue is fixed [upstream](https://gitlab.torproject.org/tpo/core/arti/-/issues/2079)
- CLI: Remove the `--debug` flag

## [3.2.0-rc.4] - 2025-10-17

- ASB + CLI + GUI: Reduce redial interval to 30s; set idle connection timeout to 2h; increase auth and multiplex timeout to 60s
- ASB: Explicitly retry publishing the Bitcoin punish transaction
- GUI + ASB: Adress to `4A1tNBcsxhQA7NkswREXTD1QGz8mRyA7fGnCzPyTwqzKdDFMNje7iHUbGhCetfVUZa1PTuZCoPKj8gnJuRrFYJ2R2CEzqbJ`. This was done because the previous donation address was a subaddress which complicates transaction building.

## [3.2.0-rc.2] - 2025-10-14

- ASB: Fix an issue where the compiled binary would not know its own version

## [3.2.0-rc.1] - 2025-10-14

- ASB: Fixed a rare race condition where it would be possible for the Monero lock step to fail but the funds to still be transferred. This would require manual intervention to recover.
- ASB: Periodically store the Monero wallet on certain events (receive money, spend money, refresh wallet) to avoid loss of metadata

## [3.1.3] - 2025-10-11

- GUI + SWAP: Fix an issue where we would fail to redeem the Monero because the wallet was not fully synchronized.
- ASB: Fix an issue where we would sometimes create Monero transaction without ensuring we were fully synchronized
- GUI: Fix an issue where swaps would not be displayed at all if the status of the Bitcoin timelock could not be fetched
- Changes squashed from release 3.0.3 - 3.1.2:
  - GUI: Fix an issue where it would always say "Wait for the application to load all required components" if Tor was disabled.
  - GUI: Remember acknowledged alerts and do not show them again.
  - RENDEZVOUS-SERVER: Fix an issue where connections would timeout immediately.
  - RENDEZVOUS-SERVER: Release a standalone rendezvous server binary.
  - GUI + SWAP + ASB: Upgrade arti (tor library) to 1.5.0. This might improve connectivity reliability.
  - ASB: Fix an issue where we would not wait between re-dials of rendezvous nodes.
  - GUI: Faster startup time by allowing parts of the application be used while other components are still initializing.
  - GUI: We now default to redeeming swaps into the internal Monero wallet, and sending Bitcoin refund into the internal Bitcoin wallet. If you want to change this behaviour go to Settings -> "Redeem Policy" and "Refund Policy".
  - GUI(Linux): Fixed an issue where the screen would be blank when launching the GUI.
  - GUI(Linux): The Linux builds are not built on Ubuntu 24.04. This mean it'll require glibc 2.39 which might not be present on some systems. If this is the case for you, please use the flatpak builds. We continue to look for a better way solution.
  - GUI: A warning will be display for makers running `<3.0.0`. Versions `2.*.*` are deprecated and do not support some essential protocols such as the new cooperative Monero redeem protocol. If you are a maker and are having issues with upgrading, please contact the developer on Matrix.
  - GUI: Clearly mark makers that have no available funds as having no available funds.
  - ORCHESTRATOR: Introduce a new `asb-tracing-logger` container within the `docker-compose.yml`. The `asb-tracing-logger` gives you access to the tracing (high verbosity) logs of your asb. Download the new `orchestrator` and run it. Then run `docker compose up -d`. The new `asb-tracing-logger` container will be created. Then run `docker compose logs -f --tail 100 asb-tracing-logger` to view detailed logging and see what is going on behind the scenes. The `asb` will continue printing less-verbose logs like before.
  - ASB: You can now configure your maker to donate a small part of swaps to funding further development of the project. This is disabled by default. You can manually enable it if you choose to do so. Set `maker.developer_tip` to a number between 0 and 1. Setting `maker.developer_tip` to `0.02` will donate 2% of each swap to the [donation address](https://github.com/eigenwallet/core?tab=readme-ov-file#donations) of the project. This is defined [here](https://github.com/eigenwallet/core/blob/ce4a85bfdd3b3fd6fbdf6c4c1ab0e1c3188b7fc2/swap-env/src/defaults.rs#L9) in the code. The tip is sent by adding an additional output to the Monero lock transaction of a swap. This means this will not impact the availability of your UTXOs (unlocked funds) as it does not require an additonal transaction. Because tips are only ever sent in Monero you maintain full privacy.
  - ASB + CLI + GUI (Testnet only): Bitcoin timelocks have been tripled. This has no affect for mainnet swaps. Blocktimes are too low on testnet to be able to test reliably.

Note: The releases 3.0.3 - 3.1.1 were squashed and merged into 3.1.2

## [3.0.2] - 2025-09-21

- Fix an issue where the released binaries for Windows where incorrect labeled as having been built for Linux

## [3.0.1] - 2025-09-19

- ASB: require Monero wallet to be fully synchronized before providing quotes
- ORCHESTRATOR: Allow re-generating `docker-compose.yml` while preserving the asb config (`config.toml`). If you've ran the `orchestrator` before you can download a newer version, run it and an updated `docker-compose.yml` will be generated (overwriting the previous file). All data and configuration options will be preserved as they are stored inside the Docker volumes and the `config.toml` file.
- GUI + CLI: Fix an issue where it'd take a long time to redeem the Monero. We did not properly skip the block scanning.
- GUI + CLI: Assume Monero double spend safety after 22 instead of after 12 blocks given the recent large re-org attacks
- ORCHESTRATOR: Change exposed mainnet port from `9839` to `9939`
- ORCHESTRATOR: We incorrectly passed the `--mainnet` flag to the `asb` binary but it is the default for the asb.
- CONTROLLER: Add a `bitcoin-seed` command to the controller. You can use it to export the descriptor of the internal Bitcoin wallet.
- CLI + GUI + ASB: Accept self-signed TLS certificates and TLS certificates with older protocol versions.

## [3.0.0-beta.10] - 2025-08-14

- GUI + CLI + ASB: Fix an issue where the Monero RPC pool would fail to build TLS handshakes over Tor

## [3.0.0-beta.9] - 2025-08-12

- ASB + CONTROLLER: Add a `monero_seed` command to the controller shell. You can use it to export the seed and restore height of the internal Monero wallet. You can use those to import the wallet into a wallet software of your own choosing.
- GUI: You can now change the Monero Node without having to restart.
- GUI: You can now export the seed phrase of the Monero wallet.
- GUI + CLI: Temporarily require a minimum of 12 confirmations for Monero transactions. Just a pre-caution given the Qubic shenanigans.
- GUI + CLI + ASB: Add `/dns4/aswap.click/tcp/8888/p2p/12D3KooWQzW52mdsLHTMu1EPiz3APumG6vGwpCuyy494MAQoEa5X`, `/dns4/getxmr.st/tcp/8888/p2p/12D3KooWHHwiz6WDThPT8cEurstomg3kDSxzL2L8pwxfyX2fpxVk` to the default list of rendezvous points

## [3.0.0-beta.8] - 2025-08-10

- GUI: Speedup startup by concurrently bootstrapping Tor and requesting the user to select a wallet
- GUI: Add white background to QR code modal to make it better scannable
- GUI + CLI + ASB: Add `/dns4/rendezvous.observer/tcp/8888/p2p/12D3KooWMjceGXrYuGuDMGrfmJxALnSDbK4km6s1i1sJEgDTgGQa` to the default list of rendezvous points
- GUI + CLI + ASB: Monero RPC pool now prioritizes nodes with pre-established TCP connections
- ASB + CONTROLLER: Add a `monero_seed` command to the controller shell. You can use it to export the seed and restore height of the internal Monero wallet. You can use those to import the wallet into a wallet software of your own choosing.

## [3.0.0-beta.6] - 2025-08-07

- GUI + CLI + ASB: The Monero RPC pool now caches TCP and Tor streams
- ASB: The default configuration has been adjusted to accept Bitcoin transactions as finalized after one block (one confirmation). Bitcoin double spends are essentially impossible for practical purposes. If one is swapping extremely large amounts, they can consider dialing `bitcoin.finality_confirmations` to `2` or `3`. This will however force the swap to take much longer to complete, and also increase the risk of a refund being made.
- ASB: The `monero.monero_node_pool` flag has been removed from the config. If you want to use the Monero Node Pool, you can now do so simply by omitting `monero.daemon_url` from the config.
- ASB: The `asb` now exposes a JSON-RPC endpoint at which it can receive commands. The `asb-controller` binary implements the client side of the JSON-RPC protocol. The JSON-RPC protocol is currently entirely read-only. This means it cannot be used to withdraw funds or change configurations. The JSON-RPC endpoint is disabled by default. It can be enabled by passing the `--rpc-bind-port 127.0.0.1:9944` and `--rpc-bind-host 127.0.0.1` flags to the `asb` binary.
- CONTROLLER: A new experimental `asb-controller` binary is now shipped. It is a CLI and REPL tool to interact with an ASB over JSON-RPC. It still has limited functionality, but will be extended in the future. Currently, it can be used to:
  - Get the primary address of th Monero wallet
  - Get the balance of the Monero wallet
  - Get the balance of the Bitcoin wallet
  - Get the list of external multiaddresses that the ASB is listening on
  - Query the currently active peer-to-peer connections
  - Get a basic list of swaps from the database
- ORCHESTRATOR: A new experiemental `orchestrator` binary is now shipped.
  - The `orchestrator` is a lightweight tool to generate a production grade environement for running ASBs.
  - It guides the user through a wizard and generates a custom [Docker compose](https://docs.docker.com/compose/) file which specifies a secure Docker environment for running ASBs.
  - This will continue to evolve over time, and we will document this thoroughly once it is more stable.
- CLI + ASB + GUI: Fixed an issue where the Monero RPC pool would not handle timeouts correctly.

## [3.0.0-beta.5] - 2025-08-04

- GUI + CLI: Fixed a potential race condition where if the user closed the app while the Bitcoin was in the process of being published, manual recovery would be required to get to a recoverable state.
- GUI + CLI + ASB: Fixed an issue where the Monero block height could not be fetched due to a bug in the Monero codebase

## [3.0.0-beta.4] - 2025-08-03

- GUI: The following rendezvous points have been added to the default list of rendezvous points. If you're running a maker, please add them to your config file (under `network.rendezvous_point`). They are operated by members of the community that have shown to be reliable. These rendezvous points act as bootstrapping nodes for peer discovery: `/dns4/eigen.center/tcp/8888/p2p/12D3KooWS5RaYJt4ANKMH4zczGVhNcw5W214e2DDYXnjs5Mx5zAT`, `/dns4/swapanarchy.cfd/tcp/8888/p2p/12D3KooWRtyVpmyvwzPYXuWyakFbRKhyXGrjhq6tP7RrBofpgQGp`, `/dns4/darkness.su/tcp/8888/p2p/12D3KooWFQAgVVS9t9UgL6v1sLprJVM7am5hFK7vy9iBCCoCBYmU`
- GUI + ASB + CLI: When using the "Monero RPC Pool" feature, we now accept self-signed TLS certificates. A lot of prominent community ran nodes use self-signed certificates. This'll be revisited once RPC-over-Tor is turned on by default. When specifying a custom node (without the RPC pool feature) we still require a valid certificate.

## [3.0.0-beta.3] - 2025-08-01

## [3.0.0-beta.2] - 2025-07-27

- GUI: Fix issue where the Monero wallet cache would be corrupted when the wallet was stored while it was refreshing.

## [3.0.0-beta] - 2025-07-18

- GUI: The GUI can now be used as a Monero wallet. You can open existing Monero wallet files that were created with `monero-wallet-cli` / `monero-wallet-rpc` / `monero-wallet-gui` / Feather Wallet. You can also generate new wallets or recover existing ones from a seed phrase. To change the restore height of a wallet, go to the "Wallet" tab and click on the "..." -> "Restore height" button. You can view your previous transactions, sync your wallet with the Blockchain and send Monero.
- GUI: The internal Bitcoin wallet and the p2p identitiy is now tied directly to the Monero wallet. The Bitcoin wallet and p2p identity is derived from the entropy of the Monero seed. The `seed.pem` file has no purpose for the GUI anymore and is only used for the CLI / ASB or when using the legacy mode of the GUI.
- GUI: The data directory has been split into multiple subdirectories. Each Monero wallet has its own data directory in `identities/<wallet-primary-address>`. That directory is used to store the swap history, the wallet cache for the Bitcoin wallet and the Tor client cache. When opening the GUI you can either select a Monero wallet to open or you can click the "No Wallet (Legacy)" button to view swaps that were started with older versions of the GUI or to get access to the Bitcoin wallet that was used in previous versions of the GUI.

## [2.5.6] - 2025-07-18

- ASB: Docker image has moved to <https://github.com/eigenwallet/core/pkgs/container/asb>
- ASB + GUI + CLI: We have renamed from _UnstoppableSwap_ to _eigenwallet_ ([why?](https://eigenwallet.org/rename.html)). We will slowly migrate the entire infrastructure to the new name.

## [2.4.5] - 2025-07-17

_Some of these CHANGELOG entires have beeb merged from 2.0.3 - 2.4.3 into this release because those releases were missing Github releases._

- ASB: Lowered the Monero lock retry timeout to 10minutes. Aftet that timeout we will start an early refund.

- GUI: Users can donate a small percentage of their swap to the projects donation address. Donations will be used to fund development. This is completely optional and **disabled** by default. Monero is used exclusively for donations, ensuring full anonymity for users. Donations are only ever send for successful swaps (not refunded ones). We clearly and transparently state where how much Monero is going before the user approves a swap.

- ASB + GUI + CLI: We now cache fee estimates for the Bitcoin wallet for up to 2 minutes. This improves the speed of fee estimation and reduces the number of requests to the Electrum servers.

- ASB + CLI + GUI: Introduce a load-balancing proxy for Monero RPC nodes that automatically discovers healthy nodes and routes requests to improve connection reliability.

- ASB: Added `monero_node_pool` boolean option to ASB config. When enabled, the ASB uses the internal Monero RPC pool instead of connecting directly to a single daemon URL, providing improved reliability and automatic failover across multiple Monero nodes.

- We now call Monero function directly (via FFI bindings) instead of using `monero-wallet-rpc`.

- ASB: Since we don't communicate with `monero-wallet-rpc` anymore, the Monero wallet's will no longer be accessible by connecting to it. If you are using the asb-docker-compose setup, run this command to migrate the wallet files from the volume of the monero-wallet-rpc container to the volume of the asb container:
  ```bash
  # On testnet
  cp /var/lib/docker/volumes/testnet_stagenet_monero-wallet-rpc-data/_data/* /var/lib/docker/volumes/testnet_testnet_asb-data/_data/monero/wallets
  # On mainnet
  cp /var/lib/docker/volumes/mainnet_mainnet_monero-wallet-rpc-data/_data/* /var/lib/docker/volumes/mainnet_mainnet_asb-data/_data/monero/wallets
  ```

- ASB: The `wallet_url` option has been removed and replaced with the optional `daemon_url`, that specifies which Monero node the asb will connect to. If not specified, the asb will connect to a known public Monero node at random.

- ASB: Add a `export-monero-wallet` command which gives the Monero wallet's seed and restore height. Export this seed into a wallet software of your own choosing to manage your Monero funds.
  The seed is a 25 word mnemonic. Example:
  ```bash
  $ asb export-monero-wallet > wallet.txt
  $ cat wallet.txt
  Seed          : novelty deodorant aloof serving fuel vipers awful segments siblings bite exquisite quick snout rising hobby trash amply recipe cinema ritual problems pram getting playful novelty
  Restore height: 3403755
  $
  ```

- Logs are now written to `stderr` (instead of `stdout`). Makers relying on piping the logs need to make sure to include the `stderr` output:

  | Before                     | After                            |
  | -------------------------- | -------------------------------- |
  | `asb logs \| my-script.sh` | `asb logs  2>&1 \| my-script.sh` |
  | `asb logs > output.txt`    | `asb logs > output.txt 2>&1`     |

- GUI: Improved peer discovery: We can now connect to multiple rendezvous points at once. We also cache peers we have previously connected to locally and will attempt to connect to them again in the future, even if they aren't registered with a rendezvous point anymore.

- ASB: We now retry for 6 hours to broadcast the early refund transaction. After that, we give up and Bob will have to wait for the timelock to expire then refund himself. If we detect that Bob has cancelled the swap, we will abort the swap on our side and let Bob refund himself.

## [2.0.3] - 2025-06-12

## [2.0.2] - 2025-06-12

- GUI: Fix issue where auto updater would not display the update
- ASB + GUI + CLI: Increase request_timeout to 7s, min_retries to 10 for Electrum load balancer

## [2.0.0] - 2025-06-12

- GUI: Build Flatpak bundle in release workflow
- docs: add instructions for verifying Tauri signature files
- docs: document new `electrum_rpc_urls` and `use_mempool_space_fee_estimation` options
- docs: Instructions for verifying GUI (Tauri) signature files

## [2.0.0-beta.2] - 2025-06-11

## [2.0.0-beta.1] - 2025-06-11

- BREAKING PROTOCOL CHANGE: Takers/GUIs running `>= 2.0.0` will not be able to initiate new swaps with makers/asbs running `< 2.0.0`. Please upgrade as soon as possible. Already started swaps from older versions are not be affected.
  - Taker and Maker now collaboratively sign a `tx_refund_early` Bitcoin transaction in the negotiation phase which allows the maker to refund the Bitcoin for the taker without having to wait for the 12h cancel timelock to expire.
  - `tx_refund_early` will only be published if the maker has not locked their Monero yet. This allows swaps to be refunded quickly if the maker doesn't have enough funds available or their daemon is not fully synced. The taker can then use the refunded Bitcoin to start a new swap.
- ASB: The maker will take Monero funds needed for ongoing swaps into consideration when making a quote. A warning will be displayed if the Monero funds do not cover all ongoing swaps.
- ASB: Return a zero quote when quoting fails instead of letting the request time out
- GUI + CLI + ASB: We now do load balancing over multiple Electrum servers. This improves the reliability of all our interactions with the Bitcoin network. When transactions are published they are broadcast to all servers in parallel.
- ASB: The `electrum_rpc_url` option has been removed. A new `electrum_rpc_urls` option has been added. Use it to specify a list of Electrum servers to use. If you want you can continue using a single server by providing a single URL. For most makers we recommend:
  - Running your own [electrs](https://github.com/romanz/electrs/) server
  - Optionally providing 2-5 fallback servers. The order of the servers does matter. Electrum servers at the front of the list have priority and will be tried first. You should place your own server at the front of the list.
  - A list of public Electrum servers can be found [here](https://1209k.com/bitcoin-eye/ele.php?chain=btc)

## [1.1.7] - 2025-06-04

- ASB: Fix an issue where the asb would quote a max_swap_amount off by a couple of piconeros

## [1.1.4] - 2025-06-04

## [1.1.3] - 2025-05-31

- The Bitcoin fee estimation is now more accurate. It uses a combination of `estimatesmartfee` from Bitcoin Core and `mempool.get_fee_histogram` from Electrum to ensure our distance from the mempool tip is appropriate. If our Electrum server doesn't support fee estimation, we use the mempool.space API. The mempool space API can be disabled using the `bitcoin.use_mempool_space_fee_estimation` option in the config file. It defaults to `true`.
- ASB: You can use the `--trace` flag to log all messages to the terminal. This is useful for debugging but shouldn't be used in production because it will log a lot of data, especially related to p2p networking and tor bootstrapping. If you want to debug issues in production, read the tracing-logs inside the data directory instead.

## [1.1.2] - 2025-05-24

- Docs: Document `external_bitcoin_address` option for using a specific
  Bitcoin address when redeeming or punishing swaps.
- Removed the JSON-RPC daemon and the `start-daemon` CLI command.
- Increased the max Bitcoin fee to up to 20% of the value of the transaction. This mitigates an issue where the Bitcoin lock transaction would not get confirmed in time on the blockchain.

## [1.1.1] - 2025-05-20

- CLI + GUI + ASB: Retry the Bitcoin wallet sync up to 15 seconds to work around transient errors.

## [1.1.0] - 2025-05-19

- GUI: Discourage swapping with makers running `< 1.1.0-rc.3` because the bdk upgrade introduced a breaking change.
- GUI: Fix an issue where the auto updater would incorrectly throw an error

## [1.1.0-rc.3] - 2025-05-18

- Breaking Change(Makers): Please complete all pending swaps, then upgrade as soon as possible. Takers might not be able to taker your offers until you upgrade your asb instance.
- CLI + ASB + GUI: We upgraded dependencies related to the Bitcoin wallet. When you boot up the new version for the first time, a migration process will be run to convert the old wallet format to the new one. This might take a few minutes. We also fixed a bug where we would generate too many unused addresses in the Bitcoin wallet which would cause the wallet to take longer to start up as time goes on.
- GUI: We display detailed progress about running background tasks (Tor bootstrapping, Bitcoin wallet sync progress, etc.)

## [1.0.0-rc.21] - 2025-05-15

## [1.0.0-rc.20] - 2025-05-14

- GUI: Added introduction flow for first-time users
- CLI + GUI: Update monero-wallet-rpc to v0.18.4.0

## [1.0.0-rc.19] - 2025-04-28

## [1.0.0-rc.18] - 2025-04-28

- GUI: Feedback submitted can be responded to by the core developers. The responses will be displayed under the "Feedback" tab.

## [1.0.0-rc.17] - 2025-04-18

- GUI: The user will now be asked to approve the swap offer again before the Bitcoin lock transaction is published. Makers should take care to only assume a swap has been accepted by the taker if the Bitcoin lock transaction is detected (`Advancing state state=bitcoin lock transaction in mempool ...`). Swaps that have been safely aborted will not be displayed in the GUI anymore.

## [1.0.0-rc.16] - 2025-04-17

- ASB: Quotes are now cached (Time-to-live of 2 minutes) to avoid overloading the maker with requests in times of high demand

## [1.0.0-rc.14] - 2025-04-16

- CI: Update Rust version to 1.80
- GUI: Update social media links

## [1.0.0-rc.13] - 2025-01-24

- Docs: Added a dedicated page for makers.
- Docs: Improved the refund and punish page.
- ASB: Fixed an issue where the ASB would silently fail if the publication of the Monero refund transaction failed.
- GUI: Add a button to open the data directory for troubleshooting purposes.

## [1.0.0-rc.12] - 2025-01-14

- GUI: Fixed a bug where the CLI wasn't passed the correct Monero node.

## [1.0.0-rc.11] - 2024-12-22

- ASB: The `history` command will now display additional information about each swap such as the amounts involved, the current state and the txid of the Bitcoin lock transaction.

## [1.0.0-rc.10] - 2024-12-05

- GUI: Release .deb installer for Debian-based systems
- ASB: The maker will now retry indefinitely to redeem the Bitcoin until the cancel timelock expires. This fixes an issue where the swap would be refunded if the maker failed to publish the redeem transaction on the first try (e.g due to a network error).
- ASB (experimental): We now listen on an onion address by default using an internal Tor client. You do not need to run a Tor daemon on your own anymore. The `tor.control_port` and `tor.socks5_port` properties in the config file have been removed. A new `tor.register_hidden_service` property has been added which when set to `true` will run a hidden service on which connections will be accepted. You can configure the number of introduction points to use by setting the `tor.hidden_service_num_intro_points` (3 - 20) property in the config file. The onion address will be advertised to all rendezvous points without having to be added to `network.external_addresses`. For now, this feature is experimental and may be unstable. We recommend you use it in combination with a clearnet address. This feature is powered by [arti](https://tpo.pages.torproject.net/core/arti/), an implementation of the Tor protocol in Rust by the Tor Project.
- CLI + GUI: We can now dial makers over `/onion3/****` addresses using the integrated Tor client.

## [1.0.0-rc.7] - 2024-11-26

- GUI: Changed terminology from "swap providers" to "makers"
- GUI: For each maker, we now display a unique deterministically generated avatar derived from the maker's public key

## [1.0.0-rc.6] - 2024-11-21

- CLI + GUI: Tor is now bundled with the application. All libp2p connections between peers are routed through Tor, if the `--enable-tor` flag is set. The `--tor-socks5-port` argument has been removed. This feature is powered by [arti](https://tpo.pages.torproject.net/core/arti/), an implementation of the Tor protocol in Rust by the Tor Project.
- CLI + GUI: At startup the wallets and tor client are started in parallel. This will speed up the startup time of the application.

## [1.0.0-rc.5] - 2024-11-19

- GUI: Set new Discord invite link to non-expired one
- GUI: Fix an issues where asbs would not be sorted correctly
- ASB: Change level of logs related to rendezvous registrations to `TRACE`

## [1.0.0-rc.4] - 2024-11-17

- GUI: Fix an issue where the AppImage would render a blank screen on some Linux systems
- ASB: We now log verbose messages to hourly rotating `tracing*.log` which are kept for 24 hours. General logs are written to `swap-all.log`.

## [1.0.0-rc.2] - 2024-11-16

- GUI: ASBs discovered via rendezvous are now prioritized if they are running the latest version
- GUI: Display up to 16 characters of the peer id of ASBs

## [1.0.0-rc.1] - 2024-11-15

## [1.0.0-alpha.3] - 2024-11-14

## [1.0.0-alpha.2] - 2024-11-14

### **GUI**

- Display a progress bar to user while we are downloading the `monero-wallet-rpc`
- Release `.app` builds for Darwin

## [1.0.0-alpha.1] - 2024-11-14

- GUI: Swaps will now be refunded as soon as the cancel timelock expires if the GUI is running but the swap dialog is not open.
- Breaking change: Increased Bitcoin refund window from 12 hours (72 blocks) to 24 hours (144 blocks) on mainnet. This change affects the default transaction configuration and requires both CLI and ASB to be updated to maintain compatibility. Earlier versions will not be able to initiate new swaps with peers running this version.
- Breaking network protocol change: The libp2p version has been upgraded to 0.53 which includes breaking network protocol changes. ASBs and CLIs will not be able to swap if one of them is on the old version.
- ASB: Transfer proofs will be repeatedly sent until they are acknowledged by the other party. This fixes a bug where it'd seem to Bob as if the Alice never locked the Monero. Forcing the swap to be refunded.
- CLI: Encrypted signatures will be repeatedly sent until they are acknowledged by the other party
- ASB: We now retry indefinitely to lock Monero funds until the swap is cancelled. This fixes an issue where we would fail to lock Monero on the first try (e.g., due to the daemon not being fully synced) and would never try again, forcing the swap to be refunded.
- ASB + CLI: You can now use the `logs` command to retrieve logs stored in the past, redacting addresses and id's using `logs --redact`.
- ASB: The `--disable-timestamp` flag has been removed
- ASB: The `history` command can now be used while the asb is running.
- Introduced a cooperative Monero redeem feature for Bob to request from Alice if Bob is punished for not refunding in time. Alice can choose to cooperate but is not obligated to do so. This change is backwards compatible. To attempt recovery, resume a swap in the "Bitcoin punished" state. Success depends on Alice being active and still having a record of the swap. Note that Alice's cooperation is voluntary and recovery is not guaranteed
- CLI: `--change-address` can now be omitted. In that case, any change is refunded to the internal bitcoin wallet.

## [0.13.2] - 2024-07-02

- CLI: Buffer received transfer proofs for later processing if we're currently running a different swap
- CLI: We now display the reason for a failed cancel-refund operation to the user (#683)

## [0.13.1] - 2024-06-10

- Add retry logic to monero-wallet-rpc wallet refresh

## [0.13.0] - 2024-05-29

- Minimum Supported Rust Version (MSRV) bumped to 1.74
- Lowered default Bitcoin confirmation target for Bob to 1 to make sure Bitcoin transactions get confirmed in time
- Added support for starting the CLI (using the `start-daemon` subcommand) as a Daemon that accepts JSON-RPC requests
- Update monero-wallet-rpc version to v0.18.3.1

## [0.12.3] - 2023-09-20

- Swap: If no Monero daemon is manually specified, we will automatically choose one from a list of public daemons by connecting to each and checking their availability.

## [0.12.2] - 2023-08-08

### Changed

- Minimum Supported Rust Version (MSRV) bumped to 1.67
- ASB can now register with multiple rendezvous nodes. The `rendezvous_point` option in `config.toml` can be a string with comma separated addresses, or a toml array of address strings.

## [0.12.1] - 2023-01-09

### Changed

- Swap: merge separate cancel/refund commands into one `cancel-and-refund` command for stuck swaps

## [0.12.0] - 2022-12-31

### Changed

- Update `bdk` library to latest version. This introduces an incompatability with previous versions due to different formats being used to exchange Bitcoin transactions
- Changed ASB to quote on Monero unlocked balance instead of total balance
- Allow `asb` to set a bitcoin address that is controlled by the asb itself to redeem/punish bitcoin to

### Added

- Allow asb config overrides using environment variables. See [1231](https://github.com/comit-network/xmr-btc-swap/pull/1231)

## [0.11.0] - 2022-08-11

### Changed

- Update from Monero v0.17.2.0 to Monero v0.18.0.0
- Change Monero nodes to [Rino tool nodes](https://community.rino.io/nodes.html)
- Always write logs as JSON to files
- Change to UTC time for log messages, due to a bug causing no logging at all to be printed (linux/macos), and an [unsoundness issue](https://docs.rs/tracing-subscriber/latest/tracing_subscriber/fmt/time/struct.LocalTime.html) with local time in [the time crate](https://github.com/time-rs/time/issues/293#issuecomment-748151025)
- Fix potential integer overflow in ASB when calculating maximum Bitcoin amount for Monero balance
- Reduce Monero locking transaction fee amount from 0.000030 to 0.000016 XMR, which is still double the current median fee as reported at [monero.how](https://www.monero.how/monero-transaction-fees)

### Added

- Adjust quote based on Bitcoin balance.
  If the max_buy_btc in the ASB config is higher than the available balance to trade, it will return the max available balance discounting the Monero locking fees. In the case the balance is lower than the min_buy_btc config it will return 0 to the CLI. If the ASB returns a quote of 0 the CLI will not allow you continue with a trade.
- Reduce required confirmations for Bitcoin transactions from 2 to 1
- Both the ASB and CLI now support the [Identify](https://github.com/libp2p/specs/blob/master/identify/README.md) protocol. This makes its version and network (testnet/mainnet) avaliable to others
- Display minimum BTC deposit required to cover the minimum quantity plus fee in the Swap CLI
- Swap CLI will check its monero-wallet-rpc version and remove it if it's older than Fluorine Fermi (0.18)

## [0.10.2] - 2021-12-25

### Changed

- Record monero wallet restore blockheight in state `SwapSetupCompleted` already.
  This solves issues where the CLI went offline after sending the BTC transaction, and the monero wallet restore blockheight being recorded after Alice locked the Monero, resulting in the generated XMR redeem wallet not detecting the transaction and reporting `No unlocked balance in the specified account`.
  This is a breaking database change!
  Swaps that were saved prior to this change may fail to load if they are in state `SwapSetupCompleted` of `BtcLocked`.
  Make sure to finish your swaps before upgrading.

## [0.10.1] - 2021-12-23

### Added

- `monero-recovery` command that can be used to print the monero address, private spend and view key so one can manually recover instances where the `monero-wallet-rpc` does not pick up the Monero funds locked up by the ASB.
  Related issue: <https://github.com/comit-network/xmr-btc-swap/issues/537>
  The command takes the swap-id as parameter.
  The swap has to be in a `BtcRedeemed` state.
  Use `--help` for more details.

## [0.10.0] - 2021-10-15

### Removed

- Support for the old sled database.
  The ASB and CLI only support the new sqlite database.
  If you haven't already, you can migrate your old data using the 0.9.0 release.

### Changed

- The ASB to no longer work as a rendezvous server.
  The ASB can still register with rendezvous server as usual.

### Fixed

- Mitigate CloseNotify bug #797 by retrying getting ScriptStatus if it fail and using a more stable public mainnet electrum server.

## [0.9.0] - 2021-10-07

### Changed

- Timestamping is now enabled by default even when the ASB is not run inside an interactive terminal.
- The `cancel`, `refund` and `punish` subcommands in ASB and CLI are run with the `--force` by default and the `--force` option has been removed.
  The force flag was used to ignore blockheight and protocol state checks.
  Users can still restart a swap with these checks using the `resume` subcommand.
- Changed log level of the "Advancing state", "Establishing Connection through Tor proxy" and "Connection through Tor established" log message from tracing to debug in the CLI.
- ASB and CLI can migrate their data to sqlite to store swaps and related data.
  This makes it easier to build applications on top of xmr-btc-swap by enabling developers to read swap information directly from the database.
  This resolved an issue where users where unable to run concurrent processes, for example, users could not print the swap history if another ASB or CLI process was running.
  The sqlite database filed is named `sqlite` and is found in the data directory.
  You can print the data directory using the `config` subcommand.
  The schema can be found here [here](swap/migrations/20210903050345_create_swaps_table.sql).

#### Database migration guide

##### Delete old data

The simplest way to migrate is to accept the loss of data and delete the old database.

1. Find the location of the old database using the `config` subcommand.
2. Delete the database
3. Run xmr-btc-swap
   xmr-btc swap will create a new sqlite database and use that from now on.

##### Preserve old data

It is possible to migrate critical data from the old db to the sqlite but there are many pitfalls.

1. Run xmr-btc-swap as you would normally
   xmr-btc-swap will try and automatically migrate your existing data to the new database.
   If the existing database contains swaps for very early releases, the migration will fail due to an incompatible schema.
2. Print out the swap history using the `history` subcommand.
3. Print out the swap history stored in the old database by also passing the `--sled` flag.
   eg. `swap-cli --sled history`
4. Compare the old and new history to see if you are happy with migration.
5. If you are unhappy with the new history you can continue to use the old database by passing the `--sled flag`

### Added

- Added a `disable-timestamp` flag to the ASB that disables timestamps from logs.
- A `config` subcommand that prints the current configuration including the data directory location.
  This feature should alleviate difficulties users were having when finding where xmr-btc-swap was storing data.
- Added `export-bitcoin-wallet` subcommand to the CLI and ASB, to print the internal bitcoin wallet descriptor.
  This will allow users to transact and monitor using external wallets.

### Removed

- The `bitcoin-target-block` argument from the `balance` subcommand on the CLI.
  This argument did not affect how the balance was calculated and was pointless.

## [0.8.3] - 2021-09-03

### Fixed

- A bug where the ASB erroneously transitioned into a punishable state upon a bitcoin transaction monitoring error.
  This could lead to a scenario where the ASB was neither able to punish, nor able to refund, so the XMR could stay locked up forever while the CLI refunded the BTC.
- A bug where the CLI erroneously transitioned into a cancel-timelock-expired state upon a bitcoin transaction monitoring error.
  This could lead to a scenario where the CLI is forced to wait for cancel, even though the cancel timelock is not yet expired and the swap could still be redeemed.

## [0.8.2] - 2021-09-01

### Added

- Add the ability to view the swap-cli bitcoin balance and withdraw.
  See issue <https://github.com/comit-network/xmr-btc-swap/issues/694>.

### Fixed

- An issue where the connection between ASB and CLI would get closed prematurely.
  The CLI expects to be connected to the ASB throughout the entire swap and hence reconnects as soon as the connection is closed.
  This resulted in a loop of connections being established but instantly closed again because the ASB deemed the connection to not be necessary.
  See issue <https://github.com/comit-network/xmr-btc-swap/issues/648>.
- An issue where the ASB was unable to use the Monero wallet in case `monero-wallet-rpc` has been restarted.
  In case no wallet is loaded when we try to interact with the `monero-wallet-rpc` daemon, we now load the correct wallet on-demand.
  See issue <https://github.com/comit-network/xmr-btc-swap/issues/652>.
- An issue where swap protocol was getting stuck trying to submit the cancel transaction.
  We were not handling the error when TxCancel submission fails.
  We also configured the electrum client to retry 5 times in order to help with this problem.
  See issues: <https://github.com/comit-network/xmr-btc-swap/issues/709> <https://github.com/comit-network/xmr-btc-swap/issues/688>, <https://github.com/comit-network/xmr-btc-swap/issues/701>.
- An issue where the ASB withdraw one bitcoin UTXO at a time instead of the whole balance.
  See issue <https://github.com/comit-network/xmr-btc-swap/issues/662>.

## [0.8.1] - 2021-08-16

### Fixed

- An occasional error where users couldn't start a swap because of `InsufficientFunds` that were off by exactly 1 satoshi.

## [0.8.0] - 2021-07-09

### Added

- Printing the deposit address to the terminal as a QR code.
  To not break automated scripts or integrations with other software, this behaviour is disabled if `--json` is passed to the application.
- Configuration setting for the websocket URL that the ASB connects to in order to receive price ticker updates.
  Can be configured manually by editing the config.toml file directly.
  It is expected that the server behind the url follows the same protocol as the [Kraken websocket api](https://docs.kraken.com/websockets/).
- Registration and discovery of ASBs using the [libp2p rendezvous protocol](https://github.com/libp2p/specs/blob/master/rendezvous/README.md).
  ASBs can register with a rendezvous node upon startup and, once registered, can be automatically discovered by the CLI using the `list-sellers` command.
  The rendezvous node address (`rendezvous_point`), as well as the ASB's external addresses (`external_addresses`) to be registered, is configured in the `network` section of the ASB config file.
  A rendezvous node is provided at `/dnsaddr/rendezvous.coblox.tech/p2p/12D3KooWQUt9DkNZxEn2R5ymJzWj15MpG6mTW84kyd8vDaRZi46o` for testing purposes.
  Upon discovery using `list-sellers` CLI users are provided with quote and connection information for each ASB discovered through the rendezvous node.
- A mandatory `--change-address` parameter to the CLI's `buy-xmr` command.
  The provided address is used to transfer Bitcoin in case of a refund and in case the user transfers more than the specified amount into the swap.
  For more information see [#513](https://github.com/comit-network/xmr-btc-swap/issues/513).

### Fixed

- An issue where the ASB gives long price guarantees when setting up a swap.
  Now, after sending a spot price the ASB will wait for one minute for the CLI's to trigger the execution setup, and three minutes to see the BTC lock transaction of the CLI in mempool after the swap started.
  If the first timeout is triggered the execution setup will be aborted, if the second timeout is triggered the swap will be safely aborted.
- An issue where the default Monero node connection string would not work, because the public nodes were moved to a different domain.
  The default monerod nodes were updated to use the [melo tool nodes](https://melo.tools/nodes.html).

### Changed

- The commandline interface of the CLI to combine `--seller-addr` and `--seller-peer-id`.
  These two parameters have been merged into a parameter `--seller` that accepts a single [multiaddress](https://docs.libp2p.io/concepts/addressing/).
  The multiaddress must end with a `/p2p` protocol defining the seller's peer ID.
- The `--data-dir` option to `--data-base-dir`.
  Previously, this option determined the final data directory, regardless of the `--testnet` flag.
  With `--data-base-dir`, a subdirectory (either `testnet` or `mainnet`) will be created under the given path.
  This allows using the same command with or without `--testnet`.

### Removed

- The websocket transport from the CLI.
  Websockets were only ever intended to be used for the ASB side to allow websites to retrieve quotes.
  The CLI can use regular TCP connections and having both - TCP and websockets - causes problems and unnecessary overhead.
- The `--seller-addr` parameter from the CLI's `resume` command.
  This information is now loaded from the database.
- The `--receive-address` parameter from the CLI's `resume` command.
  This information is now loaded from the database.

## [0.7.0] - 2021-05-28

### Fixed

- An issue where long-running connections are dead without a connection closure being reported back to the swarm.
  Adding a periodic ping ensures that the connection is kept alive, and a broken connection is reported back resulting in a close event on the swarm.
  This fixes the error of the ASB being unable to send a transfer proof to the CLI.
- An issue where ASB Bitcoin withdrawal can be done to an address on the wrong network.
  A network check was added that compares the wallet's network against the network of the given address when building the transaction.

## [0.6.0] - 2021-05-24

### Added

- Cancel command for the ASB that allows cancelling a specific swap by id.
  Using the cancel command requires the cancel timelock to be expired, but `--force` can be used to circumvent this check.
- Refund command for the ASB that allows refunding a specific swap by id.
  Using the refund command to refund the XMR locked by the ASB requires the CLI to first refund the BTC of the swap.
  If the BTC was not refunded yet the command will print an error accordingly.
  The command has a `--force` flag that allows executing the command without checking for cancel constraints.
- Punish command for the ASB that allows punishing a specific swap by id.
  Includes a `--force` parameter that when set disables the punish timelock check and verifying that the swap is in a cancelled state already.
- Abort command for the ASB that allows safely aborting a specific swap.
  Only swaps in a state prior to locking XMR can be safely aborted.
- Redeem command for the ASB that allows redeeming a specific swap.
  Only swaps where we learned the encrypted signature are redeemable.
  The command checks for expired timelocks to ensure redeeming is safe, but the timelock check can be disable using the `--force` flag.
  By default we wait for finality of the redeem transaction; this can be disabled by setting `--do-not-await-finality`.
- Resume-only mode for the ASB.
  When started with `--resume-only` the ASB does not accept new, incoming swap requests but only finishes swaps that are resumed upon startup.
- A minimum accepted Bitcoin amount for the ASB similar to the maximum amount already present.
  For the CLI the minimum amount is enforced by waiting until at least the minimum is available as max-giveable amount.
- Added a new argument to ASB: `--json` or `-j`. If set, log messages will be printed in JSON format.

### Fixed

- An issue where both the ASB and the CLI point to the same default directory `xmr-btc-swap` for storing data.
  The asb now uses `xmr-btc-swap/asb` and the CLI `xmr-btc-swap/cli` as default directory.
  This is a breaking change.
  If you want to access data created by a previous version you will have to rename the data folder or one of the following:
  1. For the CLI you can use `--data-dir` to point to the old directory.
  2. For the ASB you can change the data-dir in the config file of the ASB.
- The CLI receives proper Error messages if setting up a swap with the ASB fails.
  This is a breaking change because the spot-price protocol response changed.
  Expected errors scenarios that are now reported back to the CLI:
  1. Balance of ASB too low
  2. Buy amount sent by CLI exceeds maximum buy amount accepted by ASB
  3. ASB is running in resume-only mode and does not accept incoming swap requests
- An issue where the monero daemon port used by the `monero-wallet-rpc` could not be specified.
  The CLI parameter `--monero-daemon-host` was changed to `--monero-daemon-address` where host and port have to be specified.
- An issue where an ASB redeem scenario can transition to a cancel and publish scenario that will fail.
  This is a breaking change for the ASB, because it introduces a new state into the database.

### Changed

- The ASB's `--max-buy` and `ask-spread` parameter were removed in favour of entries in the config file.
  The initial setup includes setting these two values now.
- From this version on the CLI and ASB run on **mainnet** by default!
  When running either application with `--testnet` Monero network defaults to `stagenet` and Bitcoin network to `testnet3`.
  This is a breaking change.
  It is recommended to run the applications with `--testnet` first and not just run the application on `mainnet` without experience.

## [0.5.0] - 2021-04-17

### Changed

- The quote protocol returns JSON encoded data instead of CBOR.
  This is a breaking change in the protocol handling, old CLI versions will not be able to process quote requests of ASBs running this version.

### Fixed

- An issue where concurrent swaps with the same peer would cause the ASB to handle network communication incorrectly.
  To fix this, all messages are now tagged with a unique identifier that is agreed upon at the start of the swap.
  This is a breaking change in the network layer and hence old versions are not compatible with this version.
  We advise to also not resume any swaps that have been created with an older version.
  It is recommended to reset / delete the database after upgrading.
- An issue where the CLI would not reconnect to the ASB in case the network connection dropped.
  We now attempt to re-establish the connection using an exponential backoff but will give up eventually after 5 minutes.

### Added

- Websocket support for the ASB.
  The ASB is now capable to listen on both TCP and Websocket connections.
  Default websocket listening port is 9940.
- Tor support as an optional feature.
  If ASB detects that Tor's control port is open, a hidden service is created for
  the network it is listening on (currently 2).
  The Tor control port as well as Tor socks5 proxy port is configurable.

## [0.4.0] - 2021-04-06

### Added

- A changelog file.
- Automatic resume of unfinished swaps for the `asb` upon startup.
  Unfinished swaps from earlier versions will be skipped.
- A configurable spread for the ASB that is applied to the asking price received from the Kraken price ticker.
  The default value is 2% and can be configured using the `--ask-spread` parameter.
  See `./asb --help` for details.

### Changed

- Require the buyer to specify the connection details of the peer they wish to swap with.
  Throughout the public demo phase of this project, the CLI traded with us by default if the peer id and multiaddress of the seller were not specified.
  Having the defaults made it easy for us to give something to the community that can easily be tested, however it is not aligned with our long-term vision of a decentralised network of sellers.
  We have removed these defaults forcing the user to specify the seller they wish to trade with.
- The `resume` command of the `swap` CLI no longer require the `--seller-peer-id` parameter.
  This information is now saved in the database.

### Fixed

- An [issue](https://github.com/comit-network/xmr-btc-swap/issues/353) where the `swap` CLI would fail on systems that were set to a locale different than English.
  A bad readiness check when waiting for `monero-wallet-rpc` to be ready caused the CLI to hang forever, preventing users from perform a swap.

### Security

- Fixed an issue where Alice would not verify if Bob's Bitcoin lock transaction is semantically correct, i.e. pays the agreed upon amount to an output owned by both of them.
  Fixing this required a **breaking change** on the network layer and hence old versions are not compatible with this version.

[unreleased]: https://github.com/eigenwallet/core/compare/4.14.0...HEAD
[4.14.0]: https://github.com/eigenwallet/core/compare/4.13.3...4.14.0
[4.13.3]: https://github.com/eigenwallet/core/compare/4.13.2...4.13.3
[4.13.2]: https://github.com/eigenwallet/core/compare/4.13.1...4.13.2
[4.13.1]: https://github.com/eigenwallet/core/compare/4.13.0...4.13.1
[4.13.0]: https://github.com/eigenwallet/core/compare/4.12.0...4.13.0
[4.12.0]: https://github.com/eigenwallet/core/compare/4.11.5...4.12.0
[4.11.5]: https://github.com/eigenwallet/core/compare/4.11.4...4.11.5
[4.11.4]: https://github.com/eigenwallet/core/compare/4.11.3...4.11.4
[4.11.3]: https://github.com/eigenwallet/core/compare/4.11.2...4.11.3
[4.11.2]: https://github.com/eigenwallet/core/compare/4.11.0...4.11.2
[4.11.0]: https://github.com/eigenwallet/core/compare/4.10.2...4.11.0
[4.10.2]: https://github.com/eigenwallet/core/compare/4.10.1...4.10.2
[4.10.1]: https://github.com/eigenwallet/core/compare/4.10.0...4.10.1
[4.10.0]: https://github.com/eigenwallet/core/compare/4.9.6...4.10.0
[4.9.6]: https://github.com/eigenwallet/core/compare/4.9.5...4.9.6
[4.9.5]: https://github.com/eigenwallet/core/compare/4.9.4...4.9.5
[4.9.4]: https://github.com/eigenwallet/core/compare/4.9.3...4.9.4
[4.9.3]: https://github.com/eigenwallet/core/compare/4.9.2...4.9.3
[4.9.2]: https://github.com/eigenwallet/core/compare/4.9.1...4.9.2
[4.9.1]: https://github.com/eigenwallet/core/compare/4.9.0...4.9.1
[4.9.0]: https://github.com/eigenwallet/core/compare/4.8.4...4.9.0
[4.8.4]: https://github.com/eigenwallet/core/compare/4.8.3...4.8.4
[4.8.3]: https://github.com/eigenwallet/core/compare/4.8.2...4.8.3
[4.8.2]: https://github.com/eigenwallet/core/compare/4.8.1...4.8.2
[4.8.1]: https://github.com/eigenwallet/core/compare/4.8.0...4.8.1
[4.8.0]: https://github.com/eigenwallet/core/compare/4.7.11...4.8.0
[4.7.11]: https://github.com/eigenwallet/core/compare/4.7.10...4.7.11
[4.7.10]: https://github.com/eigenwallet/core/compare/3.7.10...4.7.10
[3.7.10]: https://github.com/eigenwallet/core/compare/4.7.9...3.7.10
[4.7.9]: https://github.com/eigenwallet/core/compare/4.7.8...4.7.9
[4.7.8]: https://github.com/eigenwallet/core/compare/4.6.7...4.7.8
[4.6.7]: https://github.com/eigenwallet/core/compare/4.6.6...4.6.7
[4.6.6]: https://github.com/eigenwallet/core/compare/4.6.5...4.6.6
[4.6.5]: https://github.com/eigenwallet/core/compare/4.6.4...4.6.5
[4.6.4]: https://github.com/eigenwallet/core/compare/4.6.3...4.6.4
[4.6.3]: https://github.com/eigenwallet/core/compare/4.6.1...4.6.3
[4.6.1]: https://github.com/eigenwallet/core/compare/4.6.0...4.6.1
[4.6.0]: https://github.com/eigenwallet/core/compare/4.5.5...4.6.0
[4.5.5]: https://github.com/eigenwallet/core/compare/4.5.4...4.5.5
[4.5.4]: https://github.com/eigenwallet/core/compare/4.5.3...4.5.4
[4.5.3]: https://github.com/eigenwallet/core/compare/4.5.2...4.5.3
[4.5.2]: https://github.com/eigenwallet/core/compare/4.5.1...4.5.2
[4.5.1]: https://github.com/eigenwallet/core/compare/4.5.0...4.5.1
[4.5.0]: https://github.com/eigenwallet/core/compare/4.4.1...4.5.0
[4.4.1]: https://github.com/eigenwallet/core/compare/4.4.0...4.4.1
[4.4.0]: https://github.com/eigenwallet/core/compare/4.3.1...4.4.0
[4.3.1]: https://github.com/eigenwallet/core/compare/4.3.0...4.3.1
[4.3.0]: https://github.com/eigenwallet/core/compare/4.2.4...4.3.0
[4.2.4]: https://github.com/eigenwallet/core/compare/4.2.3...4.2.4
[4.2.3]: https://github.com/eigenwallet/core/compare/4.2.2...4.2.3
[4.2.2]: https://github.com/eigenwallet/core/compare/4.2.1...4.2.2
[4.2.1]: https://github.com/eigenwallet/core/compare/4.2.0...4.2.1
[4.2.0]: https://github.com/eigenwallet/core/compare/4.1.1...4.2.0
[4.1.1]: https://github.com/eigenwallet/core/compare/4.1.0...4.1.1
[4.1.0]: https://github.com/eigenwallet/core/compare/4.0.5...4.1.0
[4.0.5]: https://github.com/eigenwallet/core/compare/4.0.4...4.0.5
[4.0.4]: https://github.com/eigenwallet/core/compare/4.0.3...4.0.4
[4.0.3]: https://github.com/eigenwallet/core/compare/4.0.2...4.0.3
[4.0.2]: https://github.com/eigenwallet/core/compare/4.0.1...4.0.2
[4.0.1]: https://github.com/eigenwallet/core/compare/4.0.0...4.0.1
[4.0.0]: https://github.com/eigenwallet/core/compare/3.7.0...4.0.0
[3.7.0]: https://github.com/eigenwallet/core/compare/3.6.7...3.7.0
[3.6.7]: https://github.com/eigenwallet/core/compare/3.6.6...3.6.7
[3.6.6]: https://github.com/eigenwallet/core/compare/3.6.4...3.6.6
[3.6.4]: https://github.com/eigenwallet/core/compare/3.6.3...3.6.4
[3.6.3]: https://github.com/eigenwallet/core/compare/3.6.2...3.6.3
[3.6.2]: https://github.com/eigenwallet/core/compare/3.6.1...3.6.2
[3.6.1]: https://github.com/eigenwallet/core/compare/3.6.0...3.6.1
[3.6.0]: https://github.com/eigenwallet/core/compare/3.5.2...3.6.0
[3.5.2]: https://github.com/eigenwallet/core/compare/3.5.1...3.5.2
[3.5.1]: https://github.com/eigenwallet/core/compare/3.5.0...3.5.1
[3.5.0]: https://github.com/eigenwallet/core/compare/3.4.2...3.5.0
[3.4.2]: https://github.com/eigenwallet/core/compare/3.4.1...3.4.2
[3.4.1]: https://github.com/eigenwallet/core/compare/3.4.0...3.4.1
[3.4.0]: https://github.com/eigenwallet/core/compare/3.3.8...3.4.0
[3.3.8]: https://github.com/eigenwallet/core/compare/3.3.7...3.3.8
[3.3.7]: https://github.com/eigenwallet/core/compare/3.3.6...3.3.7
[3.3.6]: https://github.com/eigenwallet/core/compare/3.3.5...3.3.6
[3.3.5]: https://github.com/eigenwallet/core/compare/3.3.4...3.3.5
[3.3.4]: https://github.com/eigenwallet/core/compare/3.3.3...3.3.4
[3.3.3]: https://github.com/eigenwallet/core/compare/3.3.2...3.3.3
[3.3.2]: https://github.com/eigenwallet/core/compare/3.3.1...3.3.2
[3.3.1]: https://github.com/eigenwallet/core/compare/3.3.0...3.3.1
[3.3.0]: https://github.com/eigenwallet/core/compare/3.2.11...3.3.0
[3.2.11]: https://github.com/eigenwallet/core/compare/3.2.10...3.2.11
[3.2.10]: https://github.com/eigenwallet/core/compare/3.2.9...3.2.10
[3.2.9]: https://github.com/eigenwallet/core/compare/3.2.8...3.2.9
[3.2.8]: https://github.com/eigenwallet/core/compare/3.2.7...3.2.8
[3.2.7]: https://github.com/eigenwallet/core/compare/3.2.6...3.2.7
[3.2.6]: https://github.com/eigenwallet/core/compare/3.2.5...3.2.6
[3.2.5]: https://github.com/eigenwallet/core/compare/3.2.4...3.2.5
[3.2.4]: https://github.com/eigenwallet/core/compare/3.2.3...3.2.4
[3.2.3]: https://github.com/eigenwallet/core/compare/3.2.2...3.2.3
[3.2.2]: https://github.com/eigenwallet/core/compare/3.2.1...3.2.2
[3.2.1]: https://github.com/eigenwallet/core/compare/3.2.0-rc.4...3.2.1
[3.2.0-rc.4]: https://github.com/eigenwallet/core/compare/3.0.0-rc.3...3.2.0-rc.4
[3.0.0-rc.3]: https://github.com/eigenwallet/core/compare/3.2.0-rc.2...3.0.0-rc.3
[3.2.0-rc.2]: https://github.com/eigenwallet/core/compare/3.2.0-rc.1...3.2.0-rc.2
[3.2.0-rc.1]: https://github.com/eigenwallet/core/compare/3.1.3...3.2.0-rc.1
[3.1.3]: https://github.com/eigenwallet/core/compare/3.1.2...3.1.3
[3.1.2]: https://github.com/eigenwallet/core/compare/3.1.1...3.1.2
[3.1.1]: https://github.com/eigenwallet/core/compare/3.1.0...3.1.1
[3.1.0]: https://github.com/eigenwallet/core/compare/3.0.7...3.1.0
[3.0.7]: https://github.com/eigenwallet/core/compare/3.0.6...3.0.7
[3.0.6]: https://github.com/eigenwallet/core/compare/3.0.5...3.0.6
[3.0.5]: https://github.com/eigenwallet/core/compare/3.0.4...3.0.5
[3.0.4]: https://github.com/eigenwallet/core/compare/3.0.3...3.0.4
[3.0.3]: https://github.com/eigenwallet/core/compare/3.0.2...3.0.3
[3.0.2]: https://github.com/eigenwallet/core/compare/3.0.1...3.0.2
[3.0.1]: https://github.com/eigenwallet/core/compare/3.0.0-beta.16...3.0.1
[3.0.0-beta.16]: https://github.com/eigenwallet/core/compare/3.0.0-beta.15...3.0.0-beta.16
[3.0.0-beta.15]: https://github.com/eigenwallet/core/compare/3.0.0-beta.14...3.0.0-beta.15
[3.0.0-beta.14]: https://github.com/eigenwallet/core/compare/3.0.0-beta.13...3.0.0-beta.14
[3.0.0-beta.13]: https://github.com/eigenwallet/core/compare/3.0.0-beta.12...3.0.0-beta.13
[3.0.0-beta.12]: https://github.com/eigenwallet/core/compare/3.0.0-beta.12...3.0.0-beta.12
[3.0.0-beta.12]: https://github.com/eigenwallet/core/compare/3.0.0-beta.11...3.0.0-beta.12
[3.0.0-beta.11]: https://github.com/eigenwallet/core/compare/3.0.0-beta.10...3.0.0-beta.11
[3.0.0-beta.10]: https://github.com/eigenwallet/core/compare/3.0.0-beta.9...3.0.0-beta.10
[3.0.0-beta.9]: https://github.com/eigenwallet/core/compare/3.0.0-beta.8...3.0.0-beta.9
[3.0.0-beta.8]: https://github.com/eigenwallet/core/compare/3.0.0-beta.7...3.0.0-beta.8
[3.0.0-beta.7]: https://github.com/eigenwallet/core/compare/3.0.0-beta.7...3.0.0-beta.7
[3.0.0-beta.7]: https://github.com/eigenwallet/core/compare/3.0.0-beta.6...3.0.0-beta.7
[3.0.0-beta.6]: https://github.com/eigenwallet/core/compare/3.0.0-beta.5...3.0.0-beta.6
[3.0.0-beta.5]: https://github.com/eigenwallet/core/compare/3.0.0-beta.4...3.0.0-beta.5
[3.0.0-beta.4]: https://github.com/eigenwallet/core/compare/3.0.0-beta.3...3.0.0-beta.4
[3.0.0-beta.3]: https://github.com/eigenwallet/core/compare/3.0.0-beta.2...3.0.0-beta.3
[3.0.0-beta.2]: https://github.com/eigenwallet/core/compare/3.0.0-beta...3.0.0-beta.2
[3.0.0-beta]: https://github.com/eigenwallet/core/compare/2.5.6...3.0.0-beta
[2.5.6]: https://github.com/eigenwallet/core/compare/2.4.5...2.5.6
[2.4.5]: https://github.com/eigenwallet/core/compare/2.4.3...2.4.5
[2.4.3]: https://github.com/eigenwallet/core/compare/2.0.3...2.4.3
[2.0.3]: https://github.com/UnstoppableSwap/core/compare/2.0.2...2.0.3
[2.0.2]: https://github.com/UnstoppableSwap/core/compare/2.0.0...2.0.2
[2.0.0]: https://github.com/UnstoppableSwap/core/compare/2.0.0-beta.2...2.0.0
[2.0.0-beta.2]: https://github.com/UnstoppableSwap/core/compare/2.0.0-beta.1...2.0.0-beta.2
[2.0.0-beta.1]: https://github.com/UnstoppableSwap/core/compare/1.1.7...2.0.0-beta.1
[1.1.7]: https://github.com/UnstoppableSwap/core/compare/1.1.4...1.1.7
[1.1.4]: https://github.com/UnstoppableSwap/core/compare/1.1.3...1.1.4
[1.1.3]: https://github.com/UnstoppableSwap/core/compare/1.1.2...1.1.3
[1.1.2]: https://github.com/UnstoppableSwap/core/compare/1.1.1...1.1.2
[1.1.1]: https://github.com/UnstoppableSwap/core/compare/1.1.0...1.1.1
[1.1.0]: https://github.com/UnstoppableSwap/core/compare/1.1.0-rc.3...1.1.0
[1.1.0-rc.3]: https://github.com/UnstoppableSwap/core/compare/1.1.0-rc.2...1.1.0-rc.3
[1.1.0-rc.2]: https://github.com/UnstoppableSwap/core/compare/1.1.0-rc.1...1.1.0-rc.2
[1.1.0-rc.1]: https://github.com/UnstoppableSwap/core/compare/1.0.0-rc.21...1.1.0-rc.1
[1.0.0-rc.21]: https://github.com/UnstoppableSwap/core/compare/1.0.0-rc.20...1.0.0-rc.21
[1.0.0-rc.20]: https://github.com/UnstoppableSwap/core/compare/1.0.0-rc.19...1.0.0-rc.20
[1.0.0-rc.19]: https://github.com/UnstoppableSwap/core/compare/1.0.0-rc.18...1.0.0-rc.19
[1.0.0-rc.18]: https://github.com/UnstoppableSwap/core/compare/1.0.0-rc.17...1.0.0-rc.18
[1.0.0-rc.17]: https://github.com/UnstoppableSwap/core/compare/1.0.0-rc.16...1.0.0-rc.17
[1.0.0-rc.16]: https://github.com/UnstoppableSwap/core/compare/1.0.0-rc.14...1.0.0-rc.16
[1.0.0-rc.14]: https://github.com/UnstoppableSwap/core/compare/1.0.0-rc.13...1.0.0-rc.14
[1.0.0-rc.13]: https://github.com/UnstoppableSwap/core/compare/1.0.0-rc.12...1.0.0-rc.13
[1.0.0-rc.12]: https://github.com/UnstoppableSwap/core/compare/1.0.0-rc.11...1.0.0-rc.12
[1.0.0-rc.11]: https://github.com/UnstoppableSwap/core/compare/1.0.0-rc.10...1.0.0-rc.11
[1.0.0-rc.10]: https://github.com/UnstoppableSwap/core/compare/1.0.0-rc.8...1.0.0-rc.10
[1.0.0-rc.8]: https://github.com/UnstoppableSwap/core/compare/1.0.0-rc.7...1.0.0-rc.8
[1.0.0-rc.7]: https://github.com/UnstoppableSwap/core/compare/1.0.0-rc.6...1.0.0-rc.7
[1.0.0-rc.6]: https://github.com/UnstoppableSwap/core/compare/1.0.0-rc.5...1.0.0-rc.6
[1.0.0-rc.5]: https://github.com/UnstoppableSwap/core/compare/1.0.0-rc.4...1.0.0-rc.5
[1.0.0-rc.4]: https://github.com/UnstoppableSwap/core/compare/1.0.0-rc.2...1.0.0-rc.4
[1.0.0-rc.2]: https://github.com/UnstoppableSwap/core/compare/1.0.0-rc.1...1.0.0-rc.2
[1.0.0-rc.1]: https://github.com/UnstoppableSwap/core/compare/1.0.0-alpha.3...1.0.0-rc.1
[1.0.0-alpha.3]: https://github.com/UnstoppableSwap/core/compare/1.0.0-alpha.2...1.0.0-alpha.3
[1.0.0-alpha.2]: https://github.com/UnstoppableSwap/core/compare/1.0.0-alpha.1...1.0.0-alpha.2
[1.0.0-alpha.1]: https://github.com/UnstoppableSwap/core/compare/0.13.2...1.0.0-alpha.1
[0.13.2]: https://github.com/comit-network/xmr-btc-swap/compare/0.13.1...0.13.2
[0.13.1]: https://github.com/comit-network/xmr-btc-swap/compare/0.13.0...0.13.1
[0.13.0]: https://github.com/comit-network/xmr-btc-swap/compare/0.12.3...0.13.0
[0.12.3]: https://github.com/comit-network/xmr-btc-swap/compare/0.12.2...0.12.3
[0.12.2]: https://github.com/comit-network/xmr-btc-swap/compare/0.12.1...0.12.2
[0.12.1]: https://github.com/comit-network/xmr-btc-swap/compare/0.12.0...0.12.1
[0.12.0]: https://github.com/comit-network/xmr-btc-swap/compare/0.11.0...0.12.0
[0.11.0]: https://github.com/comit-network/xmr-btc-swap/compare/0.10.2...0.11.0
[0.10.2]: https://github.com/comit-network/xmr-btc-swap/compare/0.10.1...0.10.2
[0.10.1]: https://github.com/comit-network/xmr-btc-swap/compare/0.10.0...0.10.1
[0.10.0]: https://github.com/comit-network/xmr-btc-swap/compare/0.9.0...0.10.0
[0.9.0]: https://github.com/comit-network/xmr-btc-swap/compare/0.8.3...0.9.0
[0.8.3]: https://github.com/comit-network/xmr-btc-swap/compare/0.8.2...0.8.3
[0.8.2]: https://github.com/comit-network/xmr-btc-swap/compare/0.8.1...0.8.2
[0.8.1]: https://github.com/comit-network/xmr-btc-swap/compare/0.8.0...0.8.1
[0.8.0]: https://github.com/comit-network/xmr-btc-swap/compare/0.7.0...0.8.0
[0.7.0]: https://github.com/comit-network/xmr-btc-swap/compare/0.6.0...0.7.0
[0.6.0]: https://github.com/comit-network/xmr-btc-swap/compare/0.5.0...0.6.0
[0.5.0]: https://github.com/comit-network/xmr-btc-swap/compare/0.4.0...0.5.0
[0.4.0]: https://github.com/comit-network/xmr-btc-swap/compare/v0.3...0.4.0
