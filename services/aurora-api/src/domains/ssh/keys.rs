//! Parsing a pasted SSH public key (C-790) and formatting `known_hosts` lines (C-792).
//! Only a single `ssh-ed25519 <base64> [comment]` line is accepted: no options prefix, no
//! certificate, no private key, no other algorithm. What is stored is rebuilt from the parsed key.
use aurora_core::{ApiError, sanitize};
use ssh_key::{Algorithm, HashAlg, PublicKey};

/// A parsed, canonicalized key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedKey {
    /// `ssh-ed25519 <base64>` (no comment, no options).
    pub canonical: String,
    /// `SHA256:<unpadded base64>` — what `ssh-keygen -lf` prints.
    pub fingerprint: String,
    /// The comment of the pasted line, sanitized to one line of at most 80 characters (may be
    /// empty).
    pub comment: String,
}

fn invalid(msg: &str) -> ApiError {
    ApiError::unprocessable("ssh_key_invalid", msg)
}

fn unsupported() -> ApiError {
    ApiError::unprocessable(
        "ssh_key_unsupported_type",
        "Only ssh-ed25519 keys are supported. Create one with `ssh-keygen -t ed25519`.",
    )
}

/// Algorithm names OpenSSH public-key lines start with, other than the one we accept.
fn is_other_key_type(token: &str) -> bool {
    token == "ssh-rsa"
        || token == "ssh-dss"
        || token.starts_with("ecdsa-sha2-")
        || token.starts_with("sk-")
}

pub fn parse_public_key(raw: &str) -> Result<ParsedKey, ApiError> {
    if raw.len() > 4096 {
        return Err(invalid("The key is too long."));
    }
    let line = raw.trim();
    if line.is_empty() {
        return Err(invalid(
            "Paste a public key (the contents of id_ed25519.pub).",
        ));
    }
    // One line only: a multi-line paste is a private key, an authorized_keys file or garbage.
    if line.contains(['\n', '\r']) {
        return Err(invalid(
            "Paste a single public key line (the contents of id_ed25519.pub), not a file.",
        ));
    }
    if line.starts_with("-----") {
        return Err(invalid(
            "That looks like a private key. Paste the public key (id_ed25519.pub) instead.",
        ));
    }
    let first = line.split_whitespace().next().unwrap_or_default();
    if first.ends_with("-cert-v01@openssh.com") {
        return Err(invalid("That is a certificate, not a public key."));
    }
    if is_other_key_type(first) {
        return Err(unsupported());
    }
    if first != "ssh-ed25519" {
        // An options prefix (`command="…" ssh-ed25519 …`), a bare blob or anything else.
        return Err(invalid(
            "Expected one line starting with `ssh-ed25519` and nothing before it.",
        ));
    }
    let key = PublicKey::from_openssh(line)
        .map_err(|_| invalid("That is not a valid ssh-ed25519 public key."))?;
    if key.algorithm() != Algorithm::Ed25519 {
        return Err(unsupported());
    }
    let comment = sanitize::one_line(key.comment().as_ref(), 80);
    let fingerprint = key.fingerprint(HashAlg::Sha256).to_string();
    let mut bare = key;
    bare.set_comment("");
    let canonical = bare
        .to_openssh()
        .map_err(|_| invalid("That is not a valid ssh-ed25519 public key."))?
        .trim()
        .to_owned();
    Ok(ParsedKey {
        canonical,
        fingerprint,
        comment: comment.trim().to_owned(),
    })
}

/// The name a new key gets: the given name, else the pasted comment, else "SSH key".
pub fn key_name(given: Option<&str>, comment: &str) -> Result<String, ApiError> {
    if let Some(g) = given
        && !g.trim().is_empty()
    {
        return aurora_core::names::clean(g, 80).map_err(|e| {
            ApiError::invalid_request(
                "Invalid request.",
                vec![aurora_core::FieldError::new("/name", e.to_string())],
            )
        });
    }
    let c = comment.trim();
    Ok(if c.is_empty() { "SSH key" } else { c }.to_owned())
}

/// One `known_hosts` line per host key (C-792): `<host> <key>` for port 22, else
/// `[<host>]:<port> <key>`.
pub fn known_hosts_lines(host: &str, port: u16, host_keys: &[String]) -> Vec<String> {
    let pattern = if port == 22 {
        host.to_owned()
    } else {
        format!("[{host}]:{port}")
    };
    host_keys.iter().map(|k| format!("{pattern} {k}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The fixture key of the SSH ITs; `FIXTURE_FP` is what `ssh-keygen -lf` prints for it
    /// (checked in, not recomputed).
    const FIXTURE: &str = "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIN5ElGab515YLw7PcnKusC/LWNtScxqkSkx78EjhVfhl alice@laptop";
    const FIXTURE_FP: &str = "SHA256:gOAeoDRLhPIlNGAongiqcd5fskHrxCKhw3I7so57P3c";

    #[test]
    fn fixture_key_is_canonicalized_with_the_ssh_keygen_fingerprint() {
        let p = parse_public_key(&format!("  {FIXTURE}\n")).unwrap();
        assert_eq!(p.fingerprint, FIXTURE_FP);
        assert_eq!(
            p.canonical,
            "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIN5ElGab515YLw7PcnKusC/LWNtScxqkSkx78EjhVfhl"
        );
        assert_eq!(p.comment, "alice@laptop");
    }

    #[test]
    fn known_hosts_for_port_22_and_other_ports() {
        let keys = vec!["ssh-ed25519 AAAA".to_owned()];
        assert_eq!(
            known_hosts_lines("ssh.example.org", 22, &keys),
            vec!["ssh.example.org ssh-ed25519 AAAA"]
        );
        assert_eq!(
            known_hosts_lines("ssh.example.org", 2222, &keys),
            vec!["[ssh.example.org]:2222 ssh-ed25519 AAAA"]
        );
        assert_eq!(known_hosts_lines("h", 22, &[]), Vec::<String>::new());
    }

    #[test]
    fn name_falls_back_to_comment_then_default() {
        assert_eq!(key_name(Some(" Work "), "c").unwrap(), "Work");
        assert_eq!(key_name(None, "alice@laptop").unwrap(), "alice@laptop");
        assert_eq!(key_name(Some("  "), "").unwrap(), "SSH key");
        assert!(key_name(Some(&"x".repeat(81)), "").is_err());
    }

    #[test]
    fn refusals_carry_their_codes() {
        let code = |s: &str| parse_public_key(s).unwrap_err().code().to_owned();
        assert_eq!(code(""), "ssh_key_invalid");
        assert_eq!(
            code("ssh-rsa AAAAB3NzaC1yc2E x"),
            "ssh_key_unsupported_type"
        );
        assert_eq!(
            code("ecdsa-sha2-nistp256 AAAA x"),
            "ssh_key_unsupported_type"
        );
        assert_eq!(
            code("sk-ssh-ed25519@openssh.com AAAA x"),
            "ssh_key_unsupported_type"
        );
        assert_eq!(
            code("ssh-ed25519-cert-v01@openssh.com AAAA x"),
            "ssh_key_invalid"
        );
        assert_eq!(
            code(
                "command=\"id\" ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIBGWjypTNLYL3ngKfpKTiXQ1eTzZPmP2FNrzx+Jnrn4/"
            ),
            "ssh_key_invalid"
        );
        assert_eq!(
            code("-----BEGIN OPENSSH PRIVATE KEY-----\nabc\n-----END OPENSSH PRIVATE KEY-----"),
            "ssh_key_invalid"
        );
        assert_eq!(code("ssh-ed25519 not-base64!!"), "ssh_key_invalid");
        assert_eq!(code("hello world"), "ssh_key_invalid");
    }
}
