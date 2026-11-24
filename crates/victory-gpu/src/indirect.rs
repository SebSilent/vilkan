//! Indirect execution's arguments, bounded before the GPU reads them. The
//! title's own compute passes write most of them; a pass we drop or
//! translate wrongly leaves whatever the memory held, and a draw of four
//! billion vertices or a dispatch past the host's limits runs until the
//! host resets the display driver. So every record is clamped on the GPU,
//! just before its execution, by a compute shader of ours (HLSL compiled by
//! the system's compiler; a 5.1 shader with its root signature inside).
//!
//! Each list also has a budget (2^28 vertex instances or threads, mesh
//! groups counted as 128): records past it run empty, so a list of
//! thousands of garbage records cannot run for seconds either.
//!
//! shortcut: the bounds (2^24 vertices, 2^26 vertex instances, 65,535
//! groups per dimension, one-dimensional mesh dispatches, the budget) cut
//! legitimate work beyond them; raise them if the title needs more.

use windows::Win32::Graphics::Direct3D::Fxc::D3DCompile;
use windows::Win32::Graphics::Direct3D::ID3DBlob;
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_R32_UINT, DXGI_FORMAT_UNKNOWN};
use windows::core::s;

use crate::host::{HostList, host, transition};

// FXC-compatible (cs_5_0): no [RootSignature] attribute (DXC-only; Proton
// ships no DXC). Typed buffers, NOT RWByteAddressBuffer: Proton's
// D3DCompile (vkd3d-shader) lacks RWByteAddressBuffer.Load/InterlockedAdd
// (probe: E5005) but supports RWBuffer<uint> and RWStructuredBuffer<uint>.
// args is a typed R32_UINT UAV we create over the argument buffer (index =
// byte/4; stride and the buffer are 4-aligned, bound() checks). budget is a
// structured uint UAV (InterlockedAdd). The root signature is serialized
// separately in make(): RootConstants(4, b0) + a UAV descriptor table
// (u0 = args, u1 = budget).
const SOURCE: &str = r#"
cbuffer Record : register(b0) { uint kind; uint stride; uint records; uint unused; };
RWBuffer<uint> args : register(u0);
// The list's work so far (vertex instances, threads): past the budget,
// records run empty.
RWStructuredBuffer<uint> budget : register(u1);
static const uint BUDGET = 1u << 28;
[numthreads(64, 1, 1)]
void main(uint3 id : SV_DispatchThreadID) {
    if (id.x >= records) return;
    uint at = (id.x * stride) / 4;
    uint cost;
    if (kind <= 1) {
        // Draws: vertex or index count, then instances.
        uint count = min(args[at], 1u << 24);
        uint instances = min(args[at + 1], (1u << 26) / max(count, 1u));
        cost = count * instances;
        args[at] = count;
        args[at + 1] = instances;
    } else if (kind == 2) {
        uint3 g = uint3(min(args[at], 65535u), min(args[at + 1], 65535u), min(args[at + 2], 65535u));
        cost = min(g.x * g.y * g.z, 1u << 20) * 64;
        args[at] = g.x;
        args[at + 1] = g.y;
        args[at + 2] = g.z;
    } else {
        // Mesh dispatches: the console's are one-dimensional.
        uint3 g = uint3(args[at], args[at + 1], args[at + 2]);
        g = uint3(min(g.x, 65535u), min(g.y, 1u), min(g.z, 1u));
        cost = g.x * g.y * g.z * 128;
        args[at] = g.x;
        args[at + 1] = g.y;
        args[at + 2] = g.z;
    }
    uint before;
    // Free-function form: vkd3d-shader lacks the .InterlockedAdd method
    // (probe E5005) but supports InterlockedAdd(budget[0], ...).
    InterlockedAdd(budget[0], cost, before);
    if (before >= BUDGET || before + cost > BUDGET) {
        // Draws keep their layout with no vertices; groups become zero.
        args[at] = 0;
    }
}
"#;

struct Clamp {
    root: ID3D12RootSignature,
    pso: ID3D12PipelineState,
    // A private 2-descriptor heap: the typed args UAV (u0) and the
    // structured budget UAV (u1), recreated per clamp in bound().
    heap: ID3D12DescriptorHeap,
    increment: u32,
}

// SAFETY: D3D12 objects are free-threaded.
unsafe impl Send for Clamp {}
// SAFETY: as above.
unsafe impl Sync for Clamp {}

fn clamp() -> Option<&'static Clamp> {
    static CLAMP: std::sync::OnceLock<Option<Clamp>> = std::sync::OnceLock::new();
    CLAMP
        .get_or_init(|| match make() {
            Ok(c) => Some(c),
            Err(e) => {
                tracing::warn!(error = %e, "no argument clamp: indirect execution dropped");
                None
            }
        })
        .as_ref()
}

fn make() -> windows::core::Result<Clamp> {
    let device = &host().ok_or_else(windows::core::Error::empty)?.device;
    let mut code: Option<ID3DBlob> = None;
    let mut errors: Option<ID3DBlob> = None;
    // SAFETY: the source and names live through the call; out pointers ours.
    let compiled = unsafe {
        D3DCompile(
            SOURCE.as_ptr().cast(),
            SOURCE.len(),
            s!("clamp"),
            None,
            None,
            s!("main"),
            s!("cs_5_0"),
            0,
            0,
            &mut code,
            Some(&mut errors),
        )
    };
    if let Err(e) = compiled {
        if let Some(errors) = errors {
            // SAFETY: the blob's own bytes.
            let text = unsafe {
                std::slice::from_raw_parts(errors.GetBufferPointer().cast::<u8>(), errors.GetBufferSize())
            };
            tracing::warn!("argument clamp: {}", String::from_utf8_lossy(text));
        }
        return Err(e);
    }
    let code = code.ok_or_else(windows::core::Error::empty)?;
    // The root signature, serialized separately (the shader has no
    // [RootSignature] attribute): RootConstants(4, b0) + a UAV descriptor
    // table (u0 = typed args, u1 = structured budget). Typed buffers need
    // descriptor UAVs, not root UAVs (which are raw byte-address).
    let range = D3D12_DESCRIPTOR_RANGE1 {
        RangeType: D3D12_DESCRIPTOR_RANGE_TYPE_UAV,
        NumDescriptors: 2,
        BaseShaderRegister: 0,
        RegisterSpace: 0,
        Flags: D3D12_DESCRIPTOR_RANGE_FLAG_DATA_VOLATILE,
        OffsetInDescriptorsFromTableStart: 0,
    };
    let params = [
        D3D12_ROOT_PARAMETER1 {
            ParameterType: D3D12_ROOT_PARAMETER_TYPE_32BIT_CONSTANTS,
            Anonymous: D3D12_ROOT_PARAMETER1_0 {
                Constants: D3D12_ROOT_CONSTANTS {
                    ShaderRegister: 0,
                    RegisterSpace: 0,
                    Num32BitValues: 4,
                },
            },
            ShaderVisibility: D3D12_SHADER_VISIBILITY_ALL,
        },
        D3D12_ROOT_PARAMETER1 {
            ParameterType: D3D12_ROOT_PARAMETER_TYPE_DESCRIPTOR_TABLE,
            Anonymous: D3D12_ROOT_PARAMETER1_0 {
                DescriptorTable: D3D12_ROOT_DESCRIPTOR_TABLE1 {
                    NumDescriptorRanges: 1,
                    pDescriptorRanges: &range,
                },
            },
            ShaderVisibility: D3D12_SHADER_VISIBILITY_ALL,
        },
    ];
    let root = crate::pipeline::host::root_signature(device, &params, &[], true)?;
    // SAFETY: the blob's own bytes; the root signature is ours, live; plain
    // heap creation and handle queries.
    unsafe {
        let bytes = std::slice::from_raw_parts(code.GetBufferPointer().cast::<u8>(), code.GetBufferSize());
        let pso = device.CreateComputePipelineState(&D3D12_COMPUTE_PIPELINE_STATE_DESC {
            pRootSignature: std::mem::transmute_copy(&root),
            CS: D3D12_SHADER_BYTECODE {
                pShaderBytecode: bytes.as_ptr().cast(),
                BytecodeLength: bytes.len(),
            },
            ..Default::default()
        })?;
        let heap = device.CreateDescriptorHeap(&D3D12_DESCRIPTOR_HEAP_DESC {
            Type: D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV,
            NumDescriptors: 2,
            Flags: D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE,
            NodeMask: 0,
        })?;
        let increment = device.GetDescriptorHandleIncrementSize(D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV);
        Ok(Clamp { root, pso, heap, increment })
    }
}

/// Records, in `list`, the clamp of `records` records of `kind` (PC
/// argument numbering) `stride` bytes apart at host address `gpu` in
/// `buffer`, and leaves the buffer ready to be read as arguments. Replaces
/// the list's compute root signature and pipeline (callers bind theirs
/// after). False if the clamp is not available.
pub(crate) fn bound(
    list: &HostList,
    buffer: &ID3D12Resource,
    gpu: u64,
    (kind, stride, records): (u32, u32, u32),
) -> bool {
    let (Some(c), Some(_budget_gpu)) = (clamp(), list.budget()) else {
        return false;
    };
    let (Some(h), Some(budget_buf)) = (host(), list.budget_resource()) else {
        return false;
    };
    if !gpu.is_multiple_of(4) || !stride.is_multiple_of(4) {
        return false;
    }
    let l = &list.list;
    let constants = [kind, stride, records, 0];
    // SAFETY: an open list; our root signature, pipeline, heap and live
    // buffers the caller's list holds.
    unsafe {
        let size = buffer.GetDesc().Width;
        let cpu = c.heap.GetCPUDescriptorHandleForHeapStart();
        let gpu_handle = c.heap.GetGPUDescriptorHandleForHeapStart();
        // args: a typed R32_UINT UAV over the argument buffer (index =
        // byte/4; the buffer and stride are 4-aligned, checked above).
        let args_desc = D3D12_UNORDERED_ACCESS_VIEW_DESC {
            Format: DXGI_FORMAT_R32_UINT,
            ViewDimension: D3D12_UAV_DIMENSION_BUFFER,
            Anonymous: D3D12_UNORDERED_ACCESS_VIEW_DESC_0 {
                Buffer: D3D12_BUFFER_UAV {
                    FirstElement: 0,
                    NumElements: (size / 4) as u32,
                    StructureByteStride: 0,
                    CounterOffsetInBytes: 0,
                    Flags: D3D12_BUFFER_UAV_FLAG_NONE,
                },
            },
        };
        h.device.CreateUnorderedAccessView(buffer, None, Some(&args_desc), cpu);
        // budget: a structured uint UAV (InterlockedAdd).
        let budget_cpu = D3D12_CPU_DESCRIPTOR_HANDLE { ptr: cpu.ptr + c.increment as usize };
        let budget_desc = D3D12_UNORDERED_ACCESS_VIEW_DESC {
            Format: DXGI_FORMAT_UNKNOWN,
            ViewDimension: D3D12_UAV_DIMENSION_BUFFER,
            Anonymous: D3D12_UNORDERED_ACCESS_VIEW_DESC_0 {
                Buffer: D3D12_BUFFER_UAV {
                    FirstElement: 0,
                    NumElements: 64,
                    StructureByteStride: 4,
                    CounterOffsetInBytes: 0,
                    Flags: D3D12_BUFFER_UAV_FLAG_NONE,
                },
            },
        };
        h.device.CreateUnorderedAccessView(&budget_buf, None, Some(&budget_desc), budget_cpu);
        // Earlier shader writes (the title's argument passes) first.
        l.ResourceBarrier(&[uav_barrier()]);
        l.SetComputeRootSignature(&c.root);
        l.SetPipelineState(&c.pso);
        l.SetComputeRoot32BitConstants(0, 4, constants.as_ptr().cast(), 0);
        l.SetDescriptorHeaps(&[Some(c.heap.clone())]);
        l.SetComputeRootDescriptorTable(1, gpu_handle);
        l.Dispatch(records.div_ceil(64), 1, 1);
        l.ResourceBarrier(&[
            uav_barrier(),
            transition(
                buffer,
                D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
                D3D12_RESOURCE_STATE_INDIRECT_ARGUMENT,
            ),
        ]);
    }
    true
}

/// After the execution: the buffer back to unordered access, where the
/// title's passes keep using it.
pub(crate) fn done(list: &HostList, buffer: &ID3D12Resource) {
    // SAFETY: an open list and a live buffer.
    unsafe {
        list.list.ResourceBarrier(&[transition(
            buffer,
            D3D12_RESOURCE_STATE_INDIRECT_ARGUMENT,
            D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
        )])
    };
}

fn uav_barrier() -> D3D12_RESOURCE_BARRIER {
    D3D12_RESOURCE_BARRIER {
        Type: D3D12_RESOURCE_BARRIER_TYPE_UAV,
        Flags: D3D12_RESOURCE_BARRIER_FLAG_NONE,
        Anonymous: D3D12_RESOURCE_BARRIER_0 {
            UAV: std::mem::ManuallyDrop::new(D3D12_RESOURCE_UAV_BARRIER { pResource: std::mem::ManuallyDrop::new(None) }),
        },
    }
}

#[cfg(test)]
mod tests {
    /// The clamp compiles with the system's compiler and the host takes it.
    #[test]
    fn argument_clamp_builds() {
        if super::host().is_none() {
            return; // no host GPU here
        }
        assert!(super::clamp().is_some());
    }
}
