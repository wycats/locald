//! Minimal virtio-mmio transport.
//!
//! This is a small MMIO register adapter for driving a `VirtioDevice` implementation.

use byteorder::{ByteOrder, LittleEndian};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use vm_memory::GuestMemoryMmap;

// VirtIO MMIO Register Offsets
const MAGIC_VALUE: u64 = 0x000;
const VERSION: u64 = 0x004;
const DEVICE_ID: u64 = 0x008;
const VENDOR_ID: u64 = 0x00c;
const DEVICE_FEATURES: u64 = 0x010;
const DEVICE_FEATURES_SEL: u64 = 0x014;
const DRIVER_FEATURES: u64 = 0x020;
const DRIVER_FEATURES_SEL: u64 = 0x024;
const QUEUE_SEL: u64 = 0x030;
const QUEUE_NUM_MAX: u64 = 0x034;
const QUEUE_NUM: u64 = 0x038;
const QUEUE_READY: u64 = 0x044;
const QUEUE_NOTIFY: u64 = 0x050;
const INTERRUPT_STATUS: u64 = 0x060;
const INTERRUPT_ACK: u64 = 0x064;
const STATUS: u64 = 0x070;
const QUEUE_DESC_LOW: u64 = 0x080;
const QUEUE_DESC_HIGH: u64 = 0x084;
const QUEUE_AVAIL_LOW: u64 = 0x090;
const QUEUE_AVAIL_HIGH: u64 = 0x094;
const QUEUE_USED_LOW: u64 = 0x0a0;
const QUEUE_USED_HIGH: u64 = 0x0a4;
const CONFIG_GENERATION: u64 = 0x0fc;
const CONFIG: u64 = 0x100;

const VENDOR_ID_VAL: u32 = 0x554d_4551; // "QEMU"

/// Interface implemented by virtio devices attached via the MMIO transport.
pub trait VirtioDevice: Send {
    /// Virtio device id (see virtio spec; e.g. 2 for block).
    fn device_type(&self) -> u32;
    /// Maximum supported queue size.
    fn queue_max_size(&self) -> u16;
    /// Offered feature bits.
    fn features(&self) -> u64;
    /// Acked/negotiated feature bits from the driver.
    fn ack_features(&mut self, features: u64);
    /// Read device config space at the given offset.
    fn read_config(&self, offset: u64, data: &mut [u8]);
    /// Write device config space at the given offset.
    fn write_config(&mut self, offset: u64, data: &[u8]);
    /// Activate device with guest memory and an interrupt callback.
    fn activate(&mut self, mem: GuestMemoryMmap, interrupt_cb: Arc<dyn Fn() + Send + Sync>);
    /// Notify a queue (kick).
    fn notify_queue(&mut self, queue_index: u16);

    /// Set queue size.
    fn set_queue_num(&mut self, queue_index: u16, num: u16);
    /// Set queue ready state.
    fn set_queue_ready(&mut self, queue_index: u16, ready: bool);
    /// Set queue descriptor table address low.
    fn set_queue_desc_low(&mut self, queue_index: u16, val: u32);
    /// Set queue descriptor table address high.
    fn set_queue_desc_high(&mut self, queue_index: u16, val: u32);
    /// Set queue available ring address low.
    fn set_queue_avail_low(&mut self, queue_index: u16, val: u32);
    /// Set queue available ring address high.
    fn set_queue_avail_high(&mut self, queue_index: u16, val: u32);
    /// Set queue used ring address low.
    fn set_queue_used_low(&mut self, queue_index: u16, val: u32);
    /// Set queue used ring address high.
    fn set_queue_used_high(&mut self, queue_index: u16, val: u32);
}

/// `VirtIO` MMIO transport adapter.
pub struct MmioTransport {
    device: Box<dyn VirtioDevice>,
    device_features_sel: u32,
    driver_features_sel: u32,
    driver_features: u64,
    queue_sel: u32,
    status: u32,
    interrupt_status: Arc<AtomicU32>,
}

impl std::fmt::Debug for MmioTransport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MmioTransport")
            .field("device_features_sel", &self.device_features_sel)
            .field("driver_features_sel", &self.driver_features_sel)
            .field("driver_features", &self.driver_features)
            .field("queue_sel", &self.queue_sel)
            .field("status", &self.status)
            .finish_non_exhaustive()
    }
}

impl MmioTransport {
    /// Create a new MMIO transport for the given device.
    pub fn new(mut device: Box<dyn VirtioDevice>, mem: GuestMemoryMmap) -> Self {
        let interrupt_status = Arc::new(AtomicU32::new(0));
        let interrupt_status_clone = interrupt_status.clone();

        let cb = Arc::new(move || {
            interrupt_status_clone.fetch_or(1, Ordering::SeqCst); // Bit 0: Used Buffer Notification
        });

        device.activate(mem, cb);

        Self {
            device,
            device_features_sel: 0,
            driver_features_sel: 0,
            driver_features: 0,
            queue_sel: 0,
            status: 0,
            interrupt_status,
        }
    }

    /// Read the interrupt status register.
    pub fn get_interrupt_status(&self) -> u32 {
        self.interrupt_status.load(Ordering::SeqCst)
    }

    /// Handle a guest read from the virtio-mmio register space.
    pub fn read(&mut self, offset: u64, data: &mut [u8]) {
        let len = data.len();
        let val = match offset {
            MAGIC_VALUE => 0x7472_6976, // "virt"
            VERSION => 2,
            DEVICE_ID => self.device.device_type(),
            VENDOR_ID => VENDOR_ID_VAL,
            DEVICE_FEATURES => {
                let features = self.device.features();
                if self.device_features_sel == 0 {
                    features as u32
                } else {
                    (features >> 32) as u32
                }
            }
            DRIVER_FEATURES => {
                if self.driver_features_sel == 0 {
                    self.driver_features as u32
                } else {
                    (self.driver_features >> 32) as u32
                }
            }
            QUEUE_NUM_MAX => u32::from(self.device.queue_max_size()),
            QUEUE_READY | CONFIG_GENERATION => 0, // TODO: expose queue ready + config generation
            INTERRUPT_STATUS => self.interrupt_status.load(Ordering::SeqCst),
            STATUS => self.status,
            _ if offset >= CONFIG => {
                self.device.read_config(offset - CONFIG, data);
                return;
            }
            _ => 0,
        };

        if len == 4 {
            LittleEndian::write_u32(data, val);
        } else if len == 1 {
            data[0] = val as u8;
        }
    }

    /// Handle a guest write to the virtio-mmio register space.
    pub fn write(&mut self, offset: u64, data: &[u8]) {
        let val = if data.len() == 4 {
            LittleEndian::read_u32(data)
        } else {
            0
        };

        match offset {
            DEVICE_FEATURES_SEL => self.device_features_sel = val,
            DRIVER_FEATURES_SEL => self.driver_features_sel = val,
            DRIVER_FEATURES => {
                let features = u64::from(val);
                if self.driver_features_sel == 0 {
                    self.driver_features = (self.driver_features & !0xFFFF_FFFF) | features;
                } else {
                    self.driver_features = (self.driver_features & 0xFFFF_FFFF) | (features << 32);
                }
                // println!("MMIO: Guest wrote DRIVER_FEATURES (sel={}): {:#x}. Total: {:#x}", self.driver_features_sel, features, self.driver_features);
                self.device.ack_features(self.driver_features);
            }
            QUEUE_SEL => self.queue_sel = val,
            QUEUE_NUM => {
                // println!("MMIO: Guest set QUEUE_NUM[{}] = {}", self.queue_sel, val);
                self.device.set_queue_num(self.queue_sel as u16, val as u16);
            }
            QUEUE_READY => {
                // println!("MMIO: Guest set QUEUE_READY[{}] = {}", self.queue_sel, val);
                self.device.set_queue_ready(self.queue_sel as u16, val == 1);
            }
            QUEUE_NOTIFY => {
                // println!("MMIO: Guest NOTIFY queue {}", val);
                self.device.notify_queue(val as u16);
            }
            INTERRUPT_ACK => {
                // println!("MMIO: Guest ACK Interrupt: {:#x}", val);
                self.interrupt_status.fetch_and(!val, Ordering::SeqCst);
            }
            STATUS => {
                // println!("MMIO Status Write: {:#x}", val);
                self.status = val;
                if val == 0 {
                    // Reset
                }
            }
            QUEUE_DESC_LOW => self.device.set_queue_desc_low(self.queue_sel as u16, val),
            QUEUE_DESC_HIGH => self.device.set_queue_desc_high(self.queue_sel as u16, val),
            QUEUE_AVAIL_LOW => self.device.set_queue_avail_low(self.queue_sel as u16, val),
            QUEUE_AVAIL_HIGH => self.device.set_queue_avail_high(self.queue_sel as u16, val),
            QUEUE_USED_LOW => self.device.set_queue_used_low(self.queue_sel as u16, val),
            QUEUE_USED_HIGH => self.device.set_queue_used_high(self.queue_sel as u16, val),
            _ if offset >= CONFIG => {
                self.device.write_config(offset - CONFIG, data);
            }
            _ => {}
        }
    }
}
