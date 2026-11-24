//! The native Vulkan implementation of the backend-neutral host trait
//! ([`vilkan_host::HostBackend`]). It drives the same record-driven execution
//! core as the ad-hoc [`crate::replay`] module ([`crate::replay::replay_record`]),
//! so a captured draw replayed through the trait is bit-identical to the ad-hoc
//! replay for the same record. The tier-A/B View-only draw needs no descriptor
//! set (the shaders read a root-view buffer from the heap via push constants).

use anyhow::Result;
use vilkan_host::{DrawRecord, DrawResources, Fault, HostBackend, ReplayResult};

use crate::Context;

/// A native Vulkan host backend bound to a device [`Context`].
pub struct VulkanBackend<'a> {
    ctx: &'a Context,
}

impl<'a> VulkanBackend<'a> {
    pub fn new(ctx: &'a Context) -> Self {
        Self { ctx }
    }
}

impl HostBackend for VulkanBackend<'_> {
    fn name(&self) -> &'static str {
        "vulkan"
    }

    fn replay(&self, record: &DrawRecord, res: &DrawResources, fault: Fault) -> Result<ReplayResult> {
        crate::replay::replay_record(self.ctx, record, res, fault)
    }
}