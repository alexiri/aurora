//! Human-string sanitization on load (SEC-18).
use aurora_core::sanitize::{block, label, one_line};

use super::model::Manifest;

fn lab(s: &mut String) {
    *s = label(s);
}
fn text(s: &mut String) {
    *s = one_line(s, 400);
}
fn opt(s: &mut Option<String>) {
    if let Some(s) = s {
        text(s);
    }
}

pub fn manifest(m: &mut Manifest) {
    lab(&mut m.package.name);
    text(&mut m.package.description);
    for c in &mut m.oauth_clients {
        opt(&mut c.setup_note);
        for f in &mut c.fields {
            lab(&mut f.label);
        }
    }
    for a in &mut m.apps {
        lab(&mut a.about.name);
        text(&mut a.about.description);
        opt(&mut a.about.label_hint);
        if let Some(n) = &mut a.about.connect_note {
            text(&mut n.text);
        }
        for c in &mut a.connect {
            use super::model::Connect;
            match c {
                Connect::Oauth2(o) => {
                    for s in &mut o.scopes {
                        lab(&mut s.name);
                    }
                    for l in &mut o.access_levels {
                        lab(&mut l.label);
                    }
                }
                Connect::Fields(f) => form(&mut f.fields),
                Connect::Hook(h) => form(&mut h.fields),
            }
        }
        if let Some(t) = &mut a.tools {
            for g in [&mut t.read, &mut t.write].into_iter().flatten() {
                if let Some(w) = &mut g.wildcard {
                    lab(&mut w.name);
                }
                for i in &mut g.items {
                    lab(&mut i.name);
                    opt(&mut i.description);
                }
            }
        }
        for ap in a.approvals.values_mut() {
            ap.title = one_line(&ap.title, 1_000);
            for f in &mut ap.fields {
                lab(&mut f.label);
            }
        }
        if let Some(ag) = &mut a.agent {
            ag.instructions = block(&ag.instructions, 1_000_000);
        }
    }
}

fn form(fields: &mut [super::model::FormField]) {
    for f in fields {
        lab(&mut f.label);
        opt(&mut f.placeholder);
        for o in &mut f.options {
            lab(&mut o.label);
            opt(&mut o.warning);
        }
    }
}
