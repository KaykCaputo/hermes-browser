# Servo GPU canvas

The MPL-2.0 servo-canvas 0.8.0 crate uses the pre-WGPU 30 return type of
BufferSlice::get_mapped_range in its optional Vello renderer. Handle the
fallible mapping and unmap the buffer on either outcome. Remove this patch
when the published renderer supports the WGPU version used by Vello.
