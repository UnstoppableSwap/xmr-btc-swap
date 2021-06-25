use libp2p::core::muxing::StreamMuxerBox;
use libp2p::core::upgrade::{SelectUpgrade, Version};
use libp2p::identity::Keypair;
use libp2p::mplex::MplexConfig;
use libp2p::ping::{Ping, PingEvent, PingSuccess};
use libp2p::swarm::{SwarmBuilder, SwarmEvent};
use libp2p::{identity, noise, yamux, Multiaddr, Swarm, Transport};
use libp2p_tor::duplex;
use libp2p_tor::torut_ext::AuthenticatedConnectionExt;
use noise::NoiseConfig;
use std::str::FromStr;
use std::time::Duration;
use torut::control::AuthenticatedConn;
use torut::onion::TorSecretKeyV3;
use tracing_subscriber::util::SubscriberInitExt;
use libp2p_tor::duplex::TorutAsyncEventHandler;
use libp2p::tcp::TokioTcpConfig;

#[tokio::main(flavor = "multi_thread")]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter("trace") // add `reqwest::connect::verbose=trace` if you want to logs of the RPC clients
        .init();

    let key = fixed_onion_identity();

    let onion_address = key
        .public()
        .get_onion_address()
        .get_address_without_dot_onion();

    tracing::info!("{}", onion_address);

    let onion_port = 7654;

    let mut client = AuthenticatedConn::new(9051).await.unwrap();

    client
        .add_ephemeral_service(&key, onion_port, onion_port)
        .await
        .unwrap();

    let mut swarm = new_swarm(client, key).await;
    let peer_id = *swarm.local_peer_id();

    tracing::info!("Peer-ID: {}", peer_id);
    // TODO: Figure out what to with the port, we could also set it to 0 and then
    // imply it from the assigned port swarm.listen_on(Multiaddr::
    // from_str(format!("/onion3/{}:{}", onion_address,
    // onion_port).as_str()).unwrap()).unwrap();
    // swarm
    //     .listen_on(
    //         Multiaddr::from_str(format!("/onion3/{}:{}", onion_address, onion_port).as_str()).unwrap(),
    //     )
    //     .unwrap();
    swarm
        .listen_on(
            Multiaddr::from_str(format!("/ip4/127.0.0.1/tcp/{}", onion_port).as_str()).unwrap(),
        )
        .unwrap();

    loop {
        match swarm.next_event().await {
            SwarmEvent::NewListenAddr(addr) => {
                tracing::info!("Listening on {}", addr);
                tracing::info!("Connection string: {}/p2p/{}", addr, peer_id);
            }
            SwarmEvent::ConnectionEstablished {
                peer_id, endpoint, ..
            } => {
                tracing::info!(
                    "Connected to {} via {}",
                    peer_id,
                    endpoint.get_remote_address()
                );
            }
            SwarmEvent::Behaviour(PingEvent { result, peer }) => match result {
                Ok(PingSuccess::Pong) => {
                    tracing::info!("Got pong from {}", peer);
                }
                Ok(PingSuccess::Ping { rtt }) => {
                    tracing::info!("Pinged {} with rtt of {}s", peer, rtt.as_secs());
                }
                Err(failure) => {
                    tracing::info!("Failed to ping {}: {}", peer, failure)
                }
            },
            event => {
                tracing::debug!("Swarm event: {:?}", event)
            }
        }
    }
}

/// Builds a new swarm that is capable of listening and dialling on the Tor
/// network.
///
/// In particular, this swarm can create ephemeral hidden services on the
/// configured Tor node.
async fn new_swarm(client: AuthenticatedConn<tokio::net::TcpStream, TorutAsyncEventHandler>, key: TorSecretKeyV3) -> Swarm<Ping> {
    let identity = fixed_libp2p_identity();

    SwarmBuilder::new(
        TokioTcpConfig::new().nodelay(true)
            .boxed()
            .upgrade(Version::V1)
            .authenticate(
                NoiseConfig::xx(
                    noise::Keypair::<noise::X25519Spec>::new()
                        .into_authentic(&identity)
                        .unwrap(),
                )
                .into_authenticated(),
            )
            .multiplex(SelectUpgrade::new(
                yamux::YamuxConfig::default(),
                MplexConfig::new(),
            ))
            .timeout(Duration::from_secs(20))
            .map(|(peer, muxer), _| (peer, StreamMuxerBox::new(muxer)))
            .boxed(),
        Ping::default(),
        identity.public().into_peer_id(),
    )
    .executor(Box::new(|f| {
        tokio::spawn(f);
    }))
    .build()
}

fn fixed_onion_identity() -> TorSecretKeyV3 {
    let fixed_onion_bytes = [
        6, 164, 217, 80, 139, 239, 11, 110, 37, 77, 191, 158, 206, 252, 178, 188, 147, 98, 54, 13,
        35, 183, 114, 231, 202, 38, 30, 29, 245, 8, 118, 153, 55, 141, 228, 109, 78, 189, 120, 28,
        172, 131, 198, 55, 113, 47, 10, 135, 139, 117, 182, 195, 46, 34, 234, 169, 85, 96, 203,
        215, 7, 155, 209, 211,
    ];
    fixed_onion_bytes.into()
}

fn fixed_libp2p_identity() -> Keypair {
    // randomly venerated bytes, corresponding peer-id:
    // 12D3KooWHKqGyK4hVtf5BQY8GpbY6fSGKDZ8eBXMQ3H2RsdnKVzC
    let fixed_identity = [
        75, 146, 26, 107, 50, 252, 71, 2, 238, 224, 92, 112, 216, 238, 131, 57, 84, 9, 218, 120,
        195, 9, 129, 102, 42, 206, 165, 102, 32, 238, 158, 248,
    ];

    let key =
        identity::ed25519::SecretKey::from_bytes(fixed_identity).expect("we always pass 32 bytes");
    identity::Keypair::Ed25519(key.into())
}
