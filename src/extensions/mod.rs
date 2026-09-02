pub(crate) mod activation;
mod extension_points;
mod reconciliation;
mod runtime_extensions;

pub(crate) use activation::{
    resolve_extension_mount_plan, ExtensionFacts, ExtensionMergeDirectory, ExtensionMount,
    ExtensionMountPlan,
};
pub(crate) use extension_points::{
    applicable_extension_refs, autodelete_extension_refs, is_hidden_related_ref,
    required_extension_refs,
};
pub(crate) use reconciliation::{reconcile_extensions, reconcile_extensions_with_metadata};
pub(crate) use runtime_extensions::runtime_checkout_dir;
