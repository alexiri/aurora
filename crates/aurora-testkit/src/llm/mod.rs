//! Mocks of the LLM providers' token endpoints and inference upstreams (`docs/18 §Additional LLM providers`).
//! Each mock verifies what the real service would (the Google token mock checks the RS256 assertion against the test
//! key) and records what it received, so a test can prove which credential reached which upstream.
pub mod aws;
pub mod azure;
pub mod bedrock;
pub mod entra;
pub mod google_token;
pub mod sts;
pub mod vertex;
