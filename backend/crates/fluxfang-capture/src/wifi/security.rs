//! WiFi AP security parsing, shared by both capture paths (monitor-mode raw
//! 802.11 RSN/WPA information elements, and managed-mode `iw scan` text) so
//! they can never disagree on how a given AP config normalizes.
//!
//! Produces a [`WifiSecurity`] — discrete `security`/`auth`/`cipher` fields
//! plus a derived human `label` — which each path injects into the beacon
//! emission payload. Nothing here touches hardware or the wall clock.

use serde_json::{json, Map, Value};

/// RSN/WPA authentication-key-management (AKM) suite (the "auth" method).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AkmSuite {
    Psk,
    Ieee8021x,
    FtPsk,
    FtIeee8021x,
    PskSha256,
    Ieee8021xSha256,
    Sae,
    FtSae,
    Owe,
    Unknown,
}

impl AkmSuite {
    /// Decode an AKM suite `type` byte under the standard `00-0F-AC` OUI.
    fn from_rsn_type(t: u8) -> AkmSuite {
        match t {
            1 => AkmSuite::Ieee8021x,
            2 => AkmSuite::Psk,
            3 => AkmSuite::FtIeee8021x,
            4 => AkmSuite::FtPsk,
            5 => AkmSuite::Ieee8021xSha256,
            6 => AkmSuite::PskSha256,
            8 => AkmSuite::Sae,
            9 => AkmSuite::FtSae,
            18 => AkmSuite::Owe,
            _ => AkmSuite::Unknown,
        }
    }
    /// Decode a WPA (vendor `00-50-F2`) AKM `type` byte (only PSK / 802.1X).
    fn from_wpa_type(t: u8) -> AkmSuite {
        match t {
            1 => AkmSuite::Ieee8021x,
            2 => AkmSuite::Psk,
            _ => AkmSuite::Unknown,
        }
    }
    /// Short normalized name used in the `auth` array.
    fn name(&self) -> Option<&'static str> {
        match self {
            AkmSuite::Psk => Some("PSK"),
            AkmSuite::Ieee8021x => Some("802.1X"),
            AkmSuite::FtPsk => Some("FT-PSK"),
            AkmSuite::FtIeee8021x => Some("FT-802.1X"),
            AkmSuite::PskSha256 => Some("PSK-SHA256"),
            AkmSuite::Ieee8021xSha256 => Some("802.1X-SHA256"),
            AkmSuite::Sae => Some("SAE"),
            AkmSuite::FtSae => Some("FT-SAE"),
            AkmSuite::Owe => Some("OWE"),
            AkmSuite::Unknown => None,
        }
    }
    fn is_wpa3(&self) -> bool {
        matches!(self, AkmSuite::Sae | AkmSuite::FtSae | AkmSuite::Owe)
    }
}

/// RSN/WPA pairwise or group cipher suite.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CipherSuite {
    Ccmp128,
    Tkip,
    Gcmp128,
    Gcmp256,
    Ccmp256,
    Wep40,
    Wep104,
    UseGroup,
    Unknown,
}

impl CipherSuite {
    fn from_type(t: u8) -> CipherSuite {
        match t {
            0 => CipherSuite::UseGroup,
            1 => CipherSuite::Wep40,
            2 => CipherSuite::Tkip,
            4 => CipherSuite::Ccmp128,
            5 => CipherSuite::Wep104,
            8 => CipherSuite::Gcmp128,
            9 => CipherSuite::Gcmp256,
            10 => CipherSuite::Ccmp256,
            _ => CipherSuite::Unknown,
        }
    }
    fn name(&self) -> Option<&'static str> {
        match self {
            CipherSuite::Ccmp128 => Some("CCMP"),
            CipherSuite::Tkip => Some("TKIP"),
            CipherSuite::Gcmp128 => Some("GCMP-128"),
            CipherSuite::Gcmp256 => Some("GCMP-256"),
            CipherSuite::Ccmp256 => Some("CCMP-256"),
            CipherSuite::Wep40 => Some("WEP-40"),
            CipherSuite::Wep104 => Some("WEP-104"),
            CipherSuite::UseGroup | CipherSuite::Unknown => None,
        }
    }
}

pub struct RsnInfo {
    pub pairwise: Vec<CipherSuite>,
    pub akm: Vec<AkmSuite>,
}
pub struct WpaInfo {
    pub pairwise: Vec<CipherSuite>,
    pub akm: Vec<AkmSuite>,
}

/// The normalized, path-agnostic security summary written into the payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WifiSecurity {
    pub security: Vec<String>,
    pub auth: Vec<String>,
    pub cipher: Vec<String>,
    pub transition_mode: bool,
    pub label: String,
}

impl WifiSecurity {
    pub fn to_json_fields(&self) -> Map<String, Value> {
        let mut m = Map::new();
        m.insert("security".into(), json!(self.security));
        m.insert("auth".into(), json!(self.auth));
        m.insert("cipher".into(), json!(self.cipher));
        m.insert("transition_mode".into(), json!(self.transition_mode));
        m.insert("security_label".into(), json!(self.label));
        m
    }
}

/// The standard `00-0F-AC` OUI prefix on RSN suite selectors.
const RSN_OUI: [u8; 3] = [0x00, 0x0f, 0xac];
/// The Microsoft WPA vendor OUI `00-50-F2` (WPA IE selectors + IE tag prefix).
const WPA_OUI: [u8; 3] = [0x00, 0x50, 0xf2];

/// Read a little-endian u16 at `off`, or `None` if out of bounds.
fn le_u16(b: &[u8], off: usize) -> Option<u16> {
    Some(u16::from_le_bytes([*b.get(off)?, *b.get(off + 1)?]))
}

/// Parse `count` 4-byte suite selectors starting at `off`, decoding each via
/// `decode` only when its 3-byte OUI matches `oui` (otherwise `Unknown`/skip
/// per the caller's mapper). Returns the decoded vec and the offset past the
/// list, or `None` if the bytes are truncated.
fn parse_selectors<T>(
    b: &[u8],
    mut off: usize,
    count: usize,
    oui: [u8; 3],
    decode: impl Fn(u8) -> T,
) -> Option<(Vec<T>, usize)> {
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        let sel = b.get(off..off + 4)?;
        // Only decode selectors under the expected OUI; a vendor-specific
        // selector under a different OUI decodes to the mapper's Unknown.
        let t = if sel[0..3] == oui { sel[3] } else { 0xff };
        out.push(decode(t));
        off += 4;
    }
    Some((out, off))
}

/// Parse an RSN IE *value* (the bytes after tag id 48 and its length byte):
/// version(2) | group cipher(4) | pairwise count(2) + suites | AKM count(2) +
/// suites | (RSN caps...). Bounds-checked throughout; returns `None` on
/// truncation before the AKM list.
pub fn parse_rsn_ie(value: &[u8]) -> Option<RsnInfo> {
    // version(2) + group cipher(4)
    let mut off = 2 + 4;
    let pw_count = le_u16(value, off)? as usize;
    off += 2;
    let (pairwise, next) =
        parse_selectors(value, off, pw_count, RSN_OUI, CipherSuite::from_type)?;
    off = next;
    let akm_count = le_u16(value, off)? as usize;
    off += 2;
    let (akm, _next) = parse_selectors(value, off, akm_count, RSN_OUI, AkmSuite::from_rsn_type)?;
    Some(RsnInfo { pairwise, akm })
}

/// Parse a vendor WPA IE *value* (bytes after tag id 221 and length). The
/// value begins with `00-50-F2-01` (OUI + WPA type 1); returns `None` if that
/// prefix is absent. Then: version(2) | group(4) | pairwise count(2)+suites |
/// AKM count(2)+suites.
pub fn parse_wpa_ie(value: &[u8]) -> Option<WpaInfo> {
    if value.get(0..3)? != WPA_OUI || *value.get(3)? != 0x01 {
        return None;
    }
    // 4 (oui+type) + version(2) + group cipher(4)
    let mut off = 4 + 2 + 4;
    let pw_count = le_u16(value, off)? as usize;
    off += 2;
    let (pairwise, next) =
        parse_selectors(value, off, pw_count, WPA_OUI, CipherSuite::from_type)?;
    off = next;
    let akm_count = le_u16(value, off)? as usize;
    off += 2;
    let (akm, _next) = parse_selectors(value, off, akm_count, WPA_OUI, AkmSuite::from_wpa_type)?;
    Some(WpaInfo { pairwise, akm })
}

/// Map an `iw scan` friendly cipher name (e.g. "CCMP", "TKIP") to a suite.
pub fn cipher_from_iw(name: &str) -> CipherSuite {
    match name.trim().to_ascii_uppercase().as_str() {
        "CCMP" | "CCMP-128" => CipherSuite::Ccmp128,
        "CCMP-256" => CipherSuite::Ccmp256,
        "TKIP" => CipherSuite::Tkip,
        "GCMP" | "GCMP-128" => CipherSuite::Gcmp128,
        "GCMP-256" => CipherSuite::Gcmp256,
        "WEP-40" | "WEP40" => CipherSuite::Wep40,
        "WEP-104" | "WEP104" => CipherSuite::Wep104,
        _ => CipherSuite::Unknown,
    }
}

/// Map an `iw scan` friendly AKM/auth name to a suite. `iw` prints e.g.
/// "PSK", "802.1X", "SAE", "PSK/SHA-256", "FT/PSK", "OWE".
pub fn akm_from_iw(name: &str) -> AkmSuite {
    match name.trim().to_ascii_uppercase().as_str() {
        "PSK" => AkmSuite::Psk,
        "802.1X" | "IEEE 802.1X" => AkmSuite::Ieee8021x,
        "FT/PSK" => AkmSuite::FtPsk,
        "FT/802.1X" => AkmSuite::FtIeee8021x,
        "PSK/SHA-256" => AkmSuite::PskSha256,
        "802.1X/SHA-256" => AkmSuite::Ieee8021xSha256,
        "SAE" => AkmSuite::Sae,
        "FT/SAE" => AkmSuite::FtSae,
        "OWE" => AkmSuite::Owe,
        _ => AkmSuite::Unknown,
    }
}

/// Dedupe-preserving push of a `&str` into `out`.
fn push_unique(out: &mut Vec<String>, s: &str) {
    let owned = s.to_string();
    if !out.contains(&owned) {
        out.push(owned);
    }
}

/// Normalize the privacy bit + optional RSN/WPA info into a [`WifiSecurity`].
pub fn normalize(privacy: bool, rsn: Option<RsnInfo>, wpa: Option<WpaInfo>) -> WifiSecurity {
    let mut security: Vec<String> = Vec::new();
    let mut auth: Vec<String> = Vec::new();
    let mut cipher: Vec<String> = Vec::new();

    if let Some(wpa) = &wpa {
        push_unique(&mut security, "WPA");
        for a in &wpa.akm {
            if let Some(n) = a.name() {
                push_unique(&mut auth, n);
            }
        }
        for c in &wpa.pairwise {
            if let Some(n) = c.name() {
                push_unique(&mut cipher, n);
            }
        }
    }
    if let Some(rsn) = &rsn {
        // RSN IE with a non-WPA3 AKM ⇒ WPA2; a WPA3-only AKM (SAE/OWE) means
        // pure WPA3, no WPA2 fallback. Mixed AKM lists get both (transition).
        if rsn.akm.is_empty() || rsn.akm.iter().any(|a| !a.is_wpa3()) {
            push_unique(&mut security, "WPA2");
        }
        if rsn.akm.iter().any(|a| a.is_wpa3()) {
            push_unique(&mut security, "WPA3");
        }
        for a in &rsn.akm {
            if let Some(n) = a.name() {
                push_unique(&mut auth, n);
            }
        }
        for c in &rsn.pairwise {
            if let Some(n) = c.name() {
                push_unique(&mut cipher, n);
            }
        }
    }

    if security.is_empty() {
        security.push(if privacy { "WEP" } else { "Open" }.to_string());
    }
    // Keep generations in a stable ascending order for a predictable label.
    security.sort_by_key(|g| match g.as_str() {
        "Open" => 0,
        "WEP" => 1,
        "WPA" => 2,
        "WPA2" => 3,
        "WPA3" => 4,
        _ => 9,
    });

    let transition_mode = security.len() > 1;
    let label = derive_label(&security, &auth, &cipher, transition_mode);
    WifiSecurity {
        security,
        auth,
        cipher,
        transition_mode,
        label,
    }
}

/// Derive the human-readable `security_label` from the normalized fields.
pub fn derive_label(
    security: &[String],
    auth: &[String],
    cipher: &[String],
    transition: bool,
) -> String {
    if security == ["Open"] {
        return "Open".to_string();
    }
    if security == ["WEP"] {
        return "WEP".to_string();
    }
    // Enhanced Open (OWE) is WPA3 with an OWE AKM and no PSK/SAE.
    let owe_only = auth.iter().any(|a| a == "OWE") && !auth.iter().any(|a| a == "PSK" || a == "SAE");
    if owe_only {
        return "Enhanced Open (OWE)".to_string();
    }
    let cipher_part = cipher.first().map(|c| format!(" ({c})")).unwrap_or_default();
    if transition {
        return format!("{}-Transition{}", security.join("/"), cipher_part);
    }
    let gen = security.first().map(String::as_str).unwrap_or("");
    let auth_part = auth_label(auth);
    format!("{gen}-{auth_part}{cipher_part}")
}

/// Pick a representative auth label: SAE > 802.1X(→Enterprise) > PSK > first.
fn auth_label(auth: &[String]) -> String {
    if auth.iter().any(|a| a == "SAE" || a == "FT-SAE") {
        return "SAE".to_string();
    }
    if auth.iter().any(|a| a.starts_with("802.1X") || a == "FT-802.1X") {
        return "Enterprise".to_string();
    }
    if auth.iter().any(|a| a == "PSK" || a == "PSK-SHA256" || a == "FT-PSK") {
        return "PSK".to_string();
    }
    auth.first().cloned().unwrap_or_else(|| "Unknown".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    // Build an RSN IE value: version 1, group CCMP, one pairwise cipher, the
    // given AKM type list. All selectors under 00-0F-AC.
    fn rsn_value(pairwise_types: &[u8], akm_types: &[u8]) -> Vec<u8> {
        let mut v = vec![0x01, 0x00]; // version 1
        v.extend_from_slice(&[0x00, 0x0f, 0xac, 4]); // group = CCMP
        v.extend_from_slice(&(pairwise_types.len() as u16).to_le_bytes());
        for t in pairwise_types {
            v.extend_from_slice(&[0x00, 0x0f, 0xac, *t]);
        }
        v.extend_from_slice(&(akm_types.len() as u16).to_le_bytes());
        for t in akm_types {
            v.extend_from_slice(&[0x00, 0x0f, 0xac, *t]);
        }
        v.extend_from_slice(&[0x00, 0x00]); // RSN caps
        v
    }

    #[test]
    fn parse_rsn_wpa2_psk_ccmp() {
        let info = parse_rsn_ie(&rsn_value(&[4], &[2])).unwrap();
        assert_eq!(info.pairwise, vec![CipherSuite::Ccmp128]);
        assert_eq!(info.akm, vec![AkmSuite::Psk]);
        let sec = normalize(true, Some(info), None);
        assert_eq!(sec.security, vec!["WPA2"]);
        assert_eq!(sec.auth, vec!["PSK"]);
        assert_eq!(sec.cipher, vec!["CCMP"]);
        assert!(!sec.transition_mode);
        assert_eq!(sec.label, "WPA2-PSK (CCMP)");
    }

    #[test]
    fn parse_rsn_wpa3_sae() {
        let info = parse_rsn_ie(&rsn_value(&[4], &[8])).unwrap(); // SAE
        let sec = normalize(true, Some(info), None);
        assert_eq!(sec.security, vec!["WPA3"]);
        assert_eq!(sec.auth, vec!["SAE"]);
        assert_eq!(sec.label, "WPA3-SAE (CCMP)");
    }

    #[test]
    fn wpa2_wpa3_transition() {
        let info = parse_rsn_ie(&rsn_value(&[4], &[2, 8])).unwrap(); // PSK + SAE
        let sec = normalize(true, Some(info), None);
        assert_eq!(sec.security, vec!["WPA2", "WPA3"]);
        assert!(sec.transition_mode);
        assert_eq!(sec.label, "WPA2/WPA3-Transition (CCMP)");
    }

    #[test]
    fn enterprise_label() {
        let info = parse_rsn_ie(&rsn_value(&[4], &[1])).unwrap(); // 802.1X
        let sec = normalize(true, Some(info), None);
        assert_eq!(sec.auth, vec!["802.1X"]);
        assert_eq!(sec.label, "WPA2-Enterprise (CCMP)");
    }

    #[test]
    fn enhanced_open_owe() {
        let info = parse_rsn_ie(&rsn_value(&[4], &[18])).unwrap(); // OWE
        let sec = normalize(false, Some(info), None);
        assert_eq!(sec.label, "Enhanced Open (OWE)");
    }

    #[test]
    fn open_and_wep() {
        let open = normalize(false, None, None);
        assert_eq!(open.security, vec!["Open"]);
        assert_eq!(open.label, "Open");
        assert!(open.auth.is_empty() && open.cipher.is_empty());
        let wep = normalize(true, None, None);
        assert_eq!(wep.security, vec!["WEP"]);
        assert_eq!(wep.label, "WEP");
    }

    #[test]
    fn wpa1_from_vendor_ie() {
        // WPA IE value: 00-50-F2-01, version 1, group TKIP, pairwise TKIP, PSK.
        let mut v = vec![0x00, 0x50, 0xf2, 0x01, 0x01, 0x00];
        v.extend_from_slice(&[0x00, 0x50, 0xf2, 2]); // group TKIP
        v.extend_from_slice(&1u16.to_le_bytes());
        v.extend_from_slice(&[0x00, 0x50, 0xf2, 2]); // pairwise TKIP
        v.extend_from_slice(&1u16.to_le_bytes());
        v.extend_from_slice(&[0x00, 0x50, 0xf2, 2]); // AKM PSK
        let info = parse_wpa_ie(&v).unwrap();
        let sec = normalize(true, None, Some(info));
        assert_eq!(sec.security, vec!["WPA"]);
        assert_eq!(sec.cipher, vec!["TKIP"]);
        assert_eq!(sec.label, "WPA-PSK (TKIP)");
    }

    #[test]
    fn truncated_rsn_returns_none() {
        assert!(parse_rsn_ie(&[0x01, 0x00, 0x00, 0x0f]).is_none());
    }

    #[test]
    fn iw_name_mappers_roundtrip_through_normalize() {
        // Scan path builds RsnInfo from iw friendly names -> same output.
        let rsn = RsnInfo {
            pairwise: vec![cipher_from_iw("CCMP")],
            akm: vec![akm_from_iw("PSK"), akm_from_iw("SAE")],
        };
        let sec = normalize(true, Some(rsn), None);
        assert_eq!(sec.label, "WPA2/WPA3-Transition (CCMP)");
    }
}
