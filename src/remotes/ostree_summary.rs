use super::RemoteRef;
use anyhow::{bail, Context, Result};
use glib::{Bytes, Checksum, ChecksumType, Variant, VariantTy};
use std::fs;
use std::path::Path;

pub(super) fn summary_digest_matches(data: &[u8], expected: &str) -> Result<bool> {
    let mut checksum = Checksum::new(ChecksumType::Sha256).context("create SHA-256 checksum")?;
    checksum.update(data);
    Ok(checksum
        .string()
        .is_some_and(|actual| actual.eq_ignore_ascii_case(expected)))
}

pub(crate) fn ref_checksum(path: &Path, ref_name: &str) -> Result<String> {
    parse_summary_refs(path)?
        .into_iter()
        .find(|candidate| candidate.name == ref_name)
        .map(|candidate| candidate.checksum)
        .with_context(|| format!("ref is not present in the remote: {ref_name}"))
}

pub(super) fn parse_summary_refs(path: &Path) -> Result<Vec<RemoteRef>> {
    let data = fs::read(path).with_context(|| format!("read {}", path.display()))?;
    parse_summary_refs_bytes(data)
        .with_context(|| format!("parse OSTree summary {}", path.display()))
}

fn parse_summary_refs_bytes(data: Vec<u8>) -> Result<Vec<RemoteRef>> {
    let variant = variant_from_bytes(data, "(a(s(taya{sv}))a{sv})")?;
    let refs_v = variant.child_value(0);
    let mut refs = Vec::with_capacity(refs_v.n_children());

    for i in 0..refs_v.n_children() {
        let item = refs_v.child_value(i);
        let name = item.child_value(0).str().unwrap_or_default().to_string();
        let info = item.child_value(1);
        refs.push(remote_ref_from_summary_info(name, &info)?);
    }

    Ok(refs)
}

pub(super) fn remote_ref_from_summary_info(name: String, info: &Variant) -> Result<RemoteRef> {
    let map = info.child_value(2);
    let (installed_size, download_size) = lookup_variant_value(&map, "xa.data")
        .filter(|data| data.n_children() == 3)
        .map(|data| {
            (
                data.child_value(0).get::<u64>().map(u64::from_be),
                data.child_value(1).get::<u64>().map(u64::from_be),
            )
        })
        .unwrap_or((None, None));
    Ok(RemoteRef {
        name,
        checksum: bytes_to_checksum(&info.child_value(1))?,
        metadata: lookup_flatpak_metadata(&map),
        download_size,
        installed_size,
    })
}

pub(super) fn parse_summary_index(path: &Path, arch: &str) -> Result<(String, Option<String>)> {
    let variant = variant_from_file(path, "(a{s(ayaaya{sv})}a{sv})")
        .with_context(|| format!("parse OSTree summary index {}", path.display()))?;
    let collection_id =
        lookup_variant_string(&variant.child_value(1), "ostree.summary.collection-id");
    let summaries = variant.child_value(0);
    for index in 0..summaries.n_children() {
        let entry = summaries.child_value(index);
        if entry.child_value(0).str() != Some(arch) {
            continue;
        }
        let details = entry.child_value(1);
        return Ok((bytes_to_checksum(&details.child_value(0))?, collection_id));
    }
    bail!("summary index has no metadata for architecture {arch}")
}

pub(super) fn parse_summary_collection_id(path: &Path) -> Result<Option<String>> {
    let variant = variant_from_file(path, "(a(s(taya{sv}))a{sv})")
        .with_context(|| format!("parse OSTree summary {}", path.display()))?;
    Ok(lookup_variant_string(
        &variant.child_value(1),
        "ostree.summary.collection-id",
    ))
}

pub(super) fn variant_from_file(path: &Path, ty: &'static str) -> Result<Variant> {
    let data = fs::read(path).with_context(|| format!("read {}", path.display()))?;
    variant_from_bytes(data, ty)
}

fn variant_from_bytes(data: Vec<u8>, ty: &'static str) -> Result<Variant> {
    let bytes = Bytes::from_owned(data);
    let ty = VariantTy::new(ty).context("invalid GVariant type")?;
    Ok(Variant::from_bytes_with_type(&bytes, ty))
}

fn lookup_variant_string(map: &Variant, key: &str) -> Option<String> {
    lookup_variant_value(map, key)?
        .str()
        .map(ToString::to_string)
}

pub(super) fn lookup_flatpak_metadata(map: &Variant) -> Option<String> {
    if let Some(metadata) = lookup_variant_string(map, "xa.metadata") {
        return Some(metadata);
    }
    let data = lookup_variant_value(map, "xa.data")?;
    if data.n_children() != 3 {
        return None;
    }
    data.child_value(2).str().map(ToString::to_string)
}

fn lookup_variant_value(map: &Variant, key: &str) -> Option<Variant> {
    for i in 0..map.n_children() {
        let entry = map.child_value(i);
        let key_variant = entry.child_value(0);
        let entry_key = key_variant.str()?;
        if entry_key != key {
            continue;
        }
        let boxed = entry.child_value(1);
        return boxed.as_variant();
    }
    None
}

fn bytes_to_checksum(variant: &Variant) -> Result<String> {
    let bytes = variant.data_as_bytes();
    let data = bytes.as_ref();
    if data.len() != 32 {
        bail!("expected 32-byte checksum, got {}", data.len());
    }
    Ok(data.iter().map(|b| format!("{b:02x}")).collect())
}
