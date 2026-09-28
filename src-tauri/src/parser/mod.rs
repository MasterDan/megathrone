// The subscription/link parser: `subscription` owns the result types and
// picks the payload shape (native sing-box config, plain links, whole-payload
// base64), `links` decodes the individual share links, `builders` assembles
// the tls/transport outbound sections, `utils` holds the helpers shared
// across those. Per the repo convention this file stays re-exports only.

mod builders;
mod links;
mod subscription;
mod utils;
#[cfg(test)]
mod parser_tests;

pub use subscription::{parse_subscription, NewEndpoint, ParsedProfile};
pub use utils::UTLS_FINGERPRINTS;
