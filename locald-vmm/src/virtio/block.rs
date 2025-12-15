use byteorder::{ByteOrder, LittleEndian};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::sync::{Arc, Mutex};
use virtio_queue::{DescriptorChain, Queue, QueueOwnedT, QueueT};
use vm_memory::{Bytes, GuestAddress, GuestMemoryMmap};

use super::mmio::VirtioDevice;

const QUEUE_SIZE: u16 = 256;
const SECTOR_SIZE: u64 = 512;

// Request types
const VIRTIO_BLK_T_IN: u32 = 0;
const VIRTIO_BLK_T_OUT: u32 = 1;
const VIRTIO_BLK_T_FLUSH: u32 = 4;

// Status
const VIRTIO_BLK_S_OK: u8 = 0;
const VIRTIO_BLK_S_IOERR: u8 = 1;
const VIRTIO_BLK_S_UNSUPP: u8 = 2;

struct QueueState {
    desc_low: u32,
    desc_high: u32,
    avail_low: u32,
    avail_high: u32,
    used_low: u32,
    used_high: u32,
}

impl QueueState {
    const fn new() -> Self {
        Self {
            desc_low: 0,
            desc_high: 0,
            avail_low: 0,
            avail_high: 0,
            used_low: 0,
            used_high: 0,
        }
    }
}

/// A minimal virtio-blk device backed by a host file.
pub struct BlockDevice {
    file: Arc<Mutex<File>>,
    queues: Vec<Mutex<Queue>>,
    queue_states: Vec<Mutex<QueueState>>,
    mem: Option<GuestMemoryMmap>,
    interrupt_cb: Option<Arc<dyn Fn() + Send + Sync>>,
    capacity: u64,
}

impl std::fmt::Debug for BlockDevice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BlockDevice")
            .field("queues", &self.queues.len())
            .field("capacity", &self.capacity)
            .finish_non_exhaustive()
    }
}

impl BlockDevice {
    /// Create a new block device backed by `file`.
    ///
    /// # Errors
    ///
    /// Returns an error if the backing file metadata cannot be read or if the
    /// virtio queue cannot be initialized.
    pub fn new(file: File) -> std::io::Result<Self> {
        let capacity = file.metadata()?.len() / SECTOR_SIZE;
        let mut queues = Vec::new();
        let mut queue_states = Vec::new();
        // We only support 1 queue for now
        let queue = Queue::new(QUEUE_SIZE).map_err(std::io::Error::other)?;
        queues.push(Mutex::new(queue));
        queue_states.push(Mutex::new(QueueState::new()));

        Ok(Self {
            file: Arc::new(Mutex::new(file)),
            queues,
            queue_states,
            mem: None,
            interrupt_cb: None,
            capacity,
        })
    }

    fn process_queue(&self, queue_index: u16) {
        let Some(mem) = &self.mem else {
            return;
        };

        let Ok(mut queue) = self.queues[queue_index as usize].lock() else {
            return;
        };

        let Ok(old_idx) = queue.used_idx(mem, std::sync::atomic::Ordering::SeqCst) else {
            return;
        };
        let mut used_desc_heads = Vec::new();

        // println!("Block: Processing queue {}", queue_index);

        let Ok(iter) = queue.iter(mem) else {
            return;
        };

        for mut chain in iter {
            // println!("Block: Found descriptor chain");
            let len = self.process_chain(&mut chain, mem);
            used_desc_heads.push((chain.head_index(), len));
        }

        if used_desc_heads.is_empty() {
            // println!("Block: No descriptors found");
        }

        for (head_index, len) in used_desc_heads {
            drop(queue.add_used(mem, head_index, len));
        }

        let Ok(new_idx) = queue.used_idx(mem, std::sync::atomic::Ordering::SeqCst) else {
            return;
        };

        // Update avail_event in Used Ring to ensure we get kicks for future requests
        // Used Ring Layout: flags(2) + idx(2) + ring(8*size) + avail_event(2)
        // Size 256. Offset = 4 + 8*256 = 4 + 2048 = 2052.
        let avail_event_addr = GuestAddress(queue.used_ring() + 4 + 8 * 256);
        let Ok(avail_idx) = queue.avail_idx(mem, std::sync::atomic::Ordering::SeqCst) else {
            return;
        };
        drop(mem.write_obj(avail_idx.0, avail_event_addr));

        let mut should_notify = false;
        if new_idx != old_idx {
            // Check Event Index logic manually
            let avail_ring = queue.avail_ring();
            drop(queue);
            let used_event_addr = GuestAddress(avail_ring + 4 + 2 * 256);
            let used_event = mem.read_obj::<u16>(used_event_addr).unwrap_or(0);

            // vring_need_event logic
            let new_w = new_idx;
            let old_w = old_idx;
            let event_w = std::num::Wrapping(used_event);
            let one_w = std::num::Wrapping(1u16);

            if (new_w - event_w - one_w) < (new_w - old_w) {
                should_notify = true;
                // println!("Block: Notify needed (EventIdx). used_event={}, new={}, old={}", used_event, new_idx, old_idx);
            } else {
                // println!("Block: Notify suppressed (EventIdx). used_event={}, new={}, old={}", used_event, new_idx, old_idx);
            }
        }

        if should_notify {
            // println!("Block: Sending interrupt");
            if let Some(cb) = &self.interrupt_cb {
                cb();
            }
        }
    }

    fn process_chain(
        &self,
        chain: &mut DescriptorChain<&GuestMemoryMmap>,
        _mem: &GuestMemoryMmap,
    ) -> u32 {
        // 1. Read Header
        let Some(head_desc) = chain.next() else {
            return 0;
        };
        let mut header = [0u8; 16];
        if chain
            .memory()
            .read_slice(&mut header, head_desc.addr())
            .is_err()
        {
            return 0;
        }

        let type_ = LittleEndian::read_u32(&header[0..4]);
        let sector = LittleEndian::read_u64(&header[8..16]);

        // println!("Block: Request Type={} Sector={}", type_, sector);

        let mut len = 0;
        let mut status = VIRTIO_BLK_S_OK;

        match type_ {
            VIRTIO_BLK_T_IN => {
                // Read from file, write to guest
                let Ok(mut file) = self.file.lock() else {
                    return 0;
                };
                if file.seek(SeekFrom::Start(sector * SECTOR_SIZE)).is_err() {
                    status = VIRTIO_BLK_S_IOERR;
                }

                let mut io_err = status != VIRTIO_BLK_S_OK;

                // The next descriptors are the data buffer
                while let Some(desc) = chain.next() {
                    if !desc.is_write_only() {
                        continue; // Should be write-only for IN request
                    }
                    if desc.len() == 1 {
                        // This is the status byte
                        drop(chain.memory().write_obj(status, desc.addr()));
                        len += 1;
                        break;
                    }

                    if !io_err {
                        let mut buf = vec![0u8; desc.len() as usize];
                        let mut offset = 0;
                        loop {
                            if offset >= buf.len() {
                                break;
                            }
                            match file.read(&mut buf[offset..]) {
                                Ok(0) => break, // EOF, rest is zeros
                                Ok(n) => offset += n,
                                Err(ref e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                                Err(_) => {
                                    status = VIRTIO_BLK_S_IOERR;
                                    io_err = true;
                                    break;
                                }
                            }
                        }

                        if !io_err {
                            if chain.memory().write_slice(&buf, desc.addr()).is_err() {
                                status = VIRTIO_BLK_S_IOERR;
                                io_err = true;
                            } else {
                                len += desc.len();
                            }
                        }
                    }
                }
            }
            VIRTIO_BLK_T_OUT => {
                // Read from guest, write to file
                let Ok(mut file) = self.file.lock() else {
                    return 0;
                };
                if file.seek(SeekFrom::Start(sector * SECTOR_SIZE)).is_err() {
                    status = VIRTIO_BLK_S_IOERR;
                }

                let mut io_err = status != VIRTIO_BLK_S_OK;

                while let Some(desc) = chain.next() {
                    if desc.is_write_only() {
                        // Status byte
                        drop(chain.memory().write_obj(status, desc.addr()));
                        len += 1;
                        break;
                    }

                    if !io_err {
                        let mut buf = vec![0u8; desc.len() as usize];
                        if chain.memory().read_slice(&mut buf, desc.addr()).is_err()
                            || file.write_all(&buf).is_err()
                        {
                            status = VIRTIO_BLK_S_IOERR;
                            io_err = true;
                        }
                    }
                }
            }
            VIRTIO_BLK_T_FLUSH => {
                if let Ok(file) = self.file.lock() {
                    if file.sync_all().is_err() {
                        status = VIRTIO_BLK_S_IOERR;
                    }
                } else {
                    status = VIRTIO_BLK_S_IOERR;
                }

                while let Some(desc) = chain.next() {
                    if desc.is_write_only() && desc.len() == 1 {
                        drop(chain.memory().write_obj(status, desc.addr()));
                        len += 1;
                        break;
                    }
                }
            }
            _ => {
                status = VIRTIO_BLK_S_UNSUPP;

                // Best-effort: write the status byte if the driver provided it.
                while let Some(desc) = chain.next() {
                    if desc.is_write_only() && desc.len() == 1 {
                        drop(chain.memory().write_obj(status, desc.addr()));
                        len += 1;
                        break;
                    }
                }
            }
        }

        len
    }
}

impl VirtioDevice for BlockDevice {
    fn device_type(&self) -> u32 {
        2 // Block
    }

    fn queue_max_size(&self) -> u16 {
        QUEUE_SIZE
    }

    fn features(&self) -> u64 {
        // VIRTIO_F_VERSION_1 (32) | VIRTIO_BLK_F_FLUSH (9) | VIRTIO_RING_F_EVENT_IDX (29)
        (1 << 32) | (1 << 9) | (1 << 29)
    }

    fn ack_features(&mut self, _features: u64) {
        // TODO: Check if VIRTIO_RING_F_EVENT_IDX was negotiated and configure queues
    }

    fn read_config(&self, offset: u64, data: &mut [u8]) {
        // Config layout:
        // 0x00: capacity (u64)
        let config_len = 8;
        if offset < config_len {
            let mut config = [0u8; 8];
            LittleEndian::write_u64(&mut config, self.capacity);

            // Copy the requested part
            let len = data.len();
            let start = offset as usize;
            if start < 8 {
                let end = std::cmp::min(start + len, 8);
                let copy_len = end - start;
                if copy_len > 0 && copy_len <= len {
                    data[0..copy_len].copy_from_slice(&config[start..end]);
                }
            }
        }
    }

    fn write_config(&mut self, _offset: u64, _data: &[u8]) {}

    fn activate(&mut self, mem: GuestMemoryMmap, interrupt_cb: Arc<dyn Fn() + Send + Sync>) {
        self.mem = Some(mem);
        self.interrupt_cb = Some(interrupt_cb);
    }

    fn notify_queue(&mut self, queue_index: u16) {
        self.process_queue(queue_index);
    }

    fn set_queue_num(&mut self, queue_index: u16, num: u16) {
        if let Some(queue_mutex) = self.queues.get(queue_index as usize) {
            let Ok(mut queue) = queue_mutex.lock() else {
                return;
            };
            queue.set_size(num);
        }
    }

    fn set_queue_ready(&mut self, queue_index: u16, ready: bool) {
        if let Some(queue_mutex) = self.queues.get(queue_index as usize) {
            let Ok(mut queue) = queue_mutex.lock() else {
                return;
            };
            queue.set_ready(ready);
            // Enable Event Index if ready (assuming negotiated for now)
            if ready {
                queue.set_event_idx(true);
            }
        }
    }

    fn set_queue_desc_low(&mut self, queue_index: u16, val: u32) {
        if let Some(queue_mutex) = self.queues.get(queue_index as usize) {
            let Ok(mut queue) = queue_mutex.lock() else {
                return;
            };
            let Ok(mut state) = self.queue_states[queue_index as usize].lock() else {
                return;
            };
            state.desc_low = val;
            queue.set_desc_table_address(Some(state.desc_low), Some(state.desc_high));
        }
    }

    fn set_queue_desc_high(&mut self, queue_index: u16, val: u32) {
        if let Some(queue_mutex) = self.queues.get(queue_index as usize) {
            let Ok(mut queue) = queue_mutex.lock() else {
                return;
            };
            let Ok(mut state) = self.queue_states[queue_index as usize].lock() else {
                return;
            };
            state.desc_high = val;
            queue.set_desc_table_address(Some(state.desc_low), Some(state.desc_high));
        }
    }

    fn set_queue_avail_low(&mut self, queue_index: u16, val: u32) {
        if let Some(queue_mutex) = self.queues.get(queue_index as usize) {
            let Ok(mut queue) = queue_mutex.lock() else {
                return;
            };
            let Ok(mut state) = self.queue_states[queue_index as usize].lock() else {
                return;
            };
            state.avail_low = val;
            queue.set_avail_ring_address(Some(state.avail_low), Some(state.avail_high));
        }
    }

    fn set_queue_avail_high(&mut self, queue_index: u16, val: u32) {
        if let Some(queue_mutex) = self.queues.get(queue_index as usize) {
            let Ok(mut queue) = queue_mutex.lock() else {
                return;
            };
            let Ok(mut state) = self.queue_states[queue_index as usize].lock() else {
                return;
            };
            state.avail_high = val;
            queue.set_avail_ring_address(Some(state.avail_low), Some(state.avail_high));
        }
    }

    fn set_queue_used_low(&mut self, queue_index: u16, val: u32) {
        if let Some(queue_mutex) = self.queues.get(queue_index as usize) {
            let Ok(mut queue) = queue_mutex.lock() else {
                return;
            };
            let Ok(mut state) = self.queue_states[queue_index as usize].lock() else {
                return;
            };
            state.used_low = val;
            queue.set_used_ring_address(Some(state.used_low), Some(state.used_high));
        }
    }

    fn set_queue_used_high(&mut self, queue_index: u16, val: u32) {
        if let Some(queue_mutex) = self.queues.get(queue_index as usize) {
            let Ok(mut queue) = queue_mutex.lock() else {
                return;
            };
            let Ok(mut state) = self.queue_states[queue_index as usize].lock() else {
                return;
            };
            state.used_high = val;
            queue.set_used_ring_address(Some(state.used_low), Some(state.used_high));
        }
    }
}
