//! The secret shared between the egress gate and the gateway process.

use std::fmt;

use rand::TryRngCore;

/// A 256-bit secret used to authenticate egress tickets.
///
/// The gate signs; the gateway verifies. Both run on this machine and the
/// secret is generated fresh whenever the daemon starts, so it never needs
/// to be stored. It is handed to the gateway process at launch.
///
/// This key does not protect data at rest. It makes the gateway's refusal
/// meaningful: without it, any process that can reach the gateway socket
/// could mint its own permission to send data to a cloud provider.
#[derive(Clone)]
pub struct TicketKey([u8; 32]);

impl TicketKey {
    /// Generate a fresh key from the operating system's random source.
    pub fn generate() -> std::io::Result<Self> {
        let mut bytes = [0u8; 32];
        rand::rngs::OsRng
            .try_fill_bytes(&mut bytes)
            .map_err(std::io::Error::other)?;
        Ok(Self(bytes))
    }

    /// Wrap 32 key bytes.
    #[must_use]
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// Key bytes, for the MAC.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// Hex encoding, for handing the key to the gateway process at launch.
    #[must_use]
    pub fn to_hex(&self) -> String {
        hex::encode(self.0)
    }

    /// Parse a hex encoding.
    pub fn from_hex(s: &str) -> Result<Self, hex::FromHexError> {
        let mut bytes = [0u8; 32];
        hex::decode_to_slice(s.trim(), &mut bytes)?;
        Ok(Self(bytes))
    }
}

impl fmt::Debug for TicketKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("TicketKey(..)")
    }
}

impl Drop for TicketKey {
    fn drop(&mut self) {
        self.0.fill(0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generate_is_random_and_hex_round_trips() {
        let a = TicketKey::generate().unwrap();
        let b = TicketKey::generate().unwrap();
        assert_ne!(a.as_bytes(), b.as_bytes());
        let hex = a.to_hex();
        assert_eq!(hex.len(), 64);
        assert_eq!(TicketKey::from_hex(&hex).unwrap().as_bytes(), a.as_bytes());
    }

    #[test]
    fn debug_never_prints_key_material() {
        let k = TicketKey::from_bytes([0xcd; 32]);
        assert_eq!(format!("{k:?}"), "TicketKey(..)");
    }
}
