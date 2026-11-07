//! Inject stage (docs/09 §5): bearer, basic, header, query, remove_header,
//! path_template, path_regex. Agent-supplied values are overwritten, never merged.

use super::Unavailable;
use super::template::{Dest, Vars, render};
use crate::inject::{Target, header_value, replace_prefixed_segment, set_query};
use aurora_registry::manifest::Inject;
use base64::{Engine, engine::general_purpose::STANDARD};
use http::{HeaderName, header};

pub fn apply_all(inject: &[Inject], v: &Vars<'_>, t: &mut Target<'_>) -> Result<(), Unavailable> {
    for i in inject {
        apply(i, v, t)?;
    }
    Ok(())
}

fn apply(i: &Inject, v: &Vars<'_>, t: &mut Target<'_>) -> Result<(), Unavailable> {
    match i {
        Inject::Bearer(tpl) => {
            let r = render(tpl, Dest::Raw, v)?;
            if r.had_empty || r.text.is_empty() {
                return Err(Unavailable);
            }
            let val =
                header_value(format!("Bearer {}", r.text).as_bytes()).map_err(|_| Unavailable)?;
            t.headers.insert(header::AUTHORIZATION, val);
        }
        Inject::Basic { user, password } => {
            let (u, p) = (render(user, Dest::Raw, v)?, render(password, Dest::Raw, v)?);
            if u.had_empty || p.had_empty {
                return Err(Unavailable);
            }
            let enc = STANDARD.encode(format!("{}:{}", u.text, p.text));
            let val = header_value(format!("Basic {enc}").as_bytes()).map_err(|_| Unavailable)?;
            t.headers.insert(header::AUTHORIZATION, val);
        }
        Inject::Header { name, value } => {
            let n = HeaderName::from_bytes(name.as_bytes()).map_err(|_| Unavailable)?;
            let r = render(value, Dest::Header, v)?;
            t.headers
                .insert(n, header_value(r.text.as_bytes()).map_err(|_| Unavailable)?);
        }
        Inject::Query { name, value } => {
            // The whole value is percent-encoded by `set_query`.
            let r = render(value, Dest::Raw, v)?;
            set_query(t.query, name, &r.text);
        }
        Inject::RemoveHeader(name) => {
            let n = HeaderName::from_bytes(name.as_bytes()).map_err(|_| Unavailable)?;
            t.headers.remove(n);
        }
        Inject::PathTemplate { prefix, value } => {
            let r = render(value, Dest::Path, v)?;
            replace_prefixed_segment(t.path, prefix, &r.text);
        }
        Inject::PathRegex {
            pattern,
            replacement,
        } => {
            let re = regex::Regex::new(pattern).map_err(|_| Unavailable)?;
            let r = render(replacement, Dest::Path, v)?;
            *t.path = re.replace(t.path.as_str(), r.text.as_str()).into_owned();
        }
    }
    Ok(())
}
