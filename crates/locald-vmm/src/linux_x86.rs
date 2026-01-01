use kvm_bindings::{KVM_PIT_SPEAKER_DUMMY, kvm_pit_config, kvm_userspace_memory_region};
use kvm_ioctls::{Kvm, VcpuExit};
use linux_loader::bootparam::boot_params;
use linux_loader::cmdline::Cmdline;
use linux_loader::configurator::linux::LinuxBootConfigurator;
use linux_loader::configurator::{BootConfigurator, BootParams};
use linux_loader::loader::elf::Elf;
use linux_loader::loader::{KernelLoader, load_cmdline};
use std::io::Write;
use std::path::Path;
use vm_memory::{Bytes, GuestAddress, GuestMemory, GuestMemoryMmap};

use crate::virtio::block::BlockDevice;
use crate::virtio::mmio::MmioTransport;

#[path = "linux/boot.rs"]
mod boot;

/// A KVM-based Virtual Machine implementation for Linux.
#[derive(Debug)]
pub struct VirtualMachine {
    kvm: Kvm,
    vm_fd: kvm_ioctls::VmFd,
    guest_mem: Option<GuestMemoryMmap>,
}

impl Default for VirtualMachine {
    fn default() -> Self {
        Self::new()
    }
}

impl VirtualMachine {
    /// Creates a new Virtual Machine instance.
    ///
    /// # Panics
    ///
    /// Panics if `/dev/kvm` cannot be opened or if VM creation fails.
    #[allow(clippy::expect_used)]
    pub fn new() -> Self {
        let kvm = Kvm::new().expect("Failed to open /dev/kvm");
        let vm_fd = kvm.create_vm().expect("Failed to create VM");

        // Enable In-Kernel Devices (PIC, PIT)
        // This provides the "motherboard" devices the kernel expects.
        vm_fd.create_irq_chip().expect("Failed to create IRQ chip");

        let pit_config = kvm_pit_config {
            flags: KVM_PIT_SPEAKER_DUMMY,
            pad: [0; 15],
        };
        vm_fd.create_pit2(pit_config).expect("Failed to create PIT");

        Self {
            kvm,
            vm_fd,
            guest_mem: None,
        }
    }

    /// Runs a Linux kernel in the VM.
    ///
    /// # Errors
    ///
    /// Returns an error if guest memory cannot be created, the kernel cannot be
    /// loaded/configured, or KVM configuration fails.
    #[allow(clippy::expect_used, clippy::print_stdout, clippy::panic)]
    pub fn run_kernel(&mut self, kernel_path: &Path, memory_mb: u64) -> std::io::Result<()> {
        // 1. Setup Memory
        let mem_size = memory_mb * 1024 * 1024;
        let guest_mem = GuestMemoryMmap::from_ranges(&[(GuestAddress(0), mem_size as usize)])
            .map_err(std::io::Error::other)?;

        // Register memory with KVM
        let userspace_addr = guest_mem
            .get_host_address(GuestAddress(0))
            .map_err(std::io::Error::other)? as u64;
        let mem_region = kvm_userspace_memory_region {
            slot: 0,
            guest_phys_addr: 0,
            memory_size: mem_size,
            userspace_addr,
            flags: 0,
        };

        #[allow(unsafe_code)]
        unsafe {
            self.vm_fd
                .set_user_memory_region(mem_region)
                .expect("Failed to set user memory region");
        }

        // 2. Load Kernel
        let mut kernel_file = std::fs::File::open(kernel_path)?;
        let kernel_loader_result =
            Elf::load(&guest_mem, None, &mut kernel_file, None).map_err(std::io::Error::other)?;

        // Setup VirtIO Block Device
        let rootfs_path = kernel_path
            .parent()
            .map(|p| p.join("rootfs.ext4"))
            .ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "kernel_path has no parent",
                )
            })?;
        let block_file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&rootfs_path)?;

        let block_device = Box::new(BlockDevice::new(block_file)?);
        let mut mmio_transport = MmioTransport::new(block_device, guest_mem.clone());

        // 3. Configure Boot Parameters (Zero Page)
        let cmdline_str = "console=ttyS0 earlyprintk=serial,ttyS0,115200 reboot=k panic=1 pci=off root=/dev/vda rw virtio_mmio.device=4K@0xd0000000:5";
        let mut cmdline = Cmdline::new(4096).map_err(std::io::Error::other)?;
        cmdline
            .insert_str(cmdline_str)
            .map_err(std::io::Error::other)?;

        let cmdline_addr = GuestAddress(0x20_000);
        load_cmdline(&guest_mem, cmdline_addr, &cmdline).map_err(std::io::Error::other)?;

        let mut params = boot_params::default();
        params.hdr.boot_flag = 0xaa55;
        params.hdr.header = 0x5372_6448;
        params.hdr.kernel_alignment = 0x0100_0000;
        params.hdr.type_of_loader = 0xff;
        params.hdr.cmd_line_ptr = cmdline_addr.0 as u32;
        params.hdr.cmdline_size = cmdline
            .as_cstring()
            .map_err(std::io::Error::other)?
            .as_bytes_with_nul()
            .len() as u32;

        // E820 Memory Map
        // We need to define at least two regions: Low Memory and High Memory.
        // The gap (0xA0000 - 0x100000) is reserved for VGA/BIOS.
        params.e820_entries = 2;

        // 1. Low Memory: 0 - 0x9F000 (636KB)
        params.e820_table[0].addr = 0;
        params.e820_table[0].size = 0x9_F000;
        params.e820_table[0].type_ = 1; // E820_RAM

        // 2. High Memory: 1MB - End
        params.e820_table[1].addr = 0x0010_0000;
        params.e820_table[1].size = mem_size - 0x0010_0000;
        params.e820_table[1].type_ = 1; // E820_RAM

        let boot_params = BootParams::new::<boot_params>(
            &params,
            GuestAddress(0x7000), // Zero Page Address
        );

        LinuxBootConfigurator::write_bootparams(&boot_params, &guest_mem)
            .map_err(std::io::Error::other)?;

        // 4. Setup Page Tables (Identity Map)
        // We use the space before the kernel (0x10000) for page tables
        Self::setup_page_tables(&guest_mem, GuestAddress(0x10000))?;

        // Setup GDT
        Self::setup_gdt(&guest_mem, GuestAddress(0x15000))?;

        self.guest_mem = Some(guest_mem);

        // 5. Create VCPU
        let mut vcpu_fd = self.vm_fd.create_vcpu(0).expect("Failed to create VCPU");

        // Setup CPUID
        let kvm_cpuid = self
            .kvm
            .get_supported_cpuid(kvm_bindings::KVM_MAX_CPUID_ENTRIES)
            .map_err(std::io::Error::other)?;
        vcpu_fd
            .set_cpuid2(&kvm_cpuid)
            .map_err(std::io::Error::other)?;

        // 6. Setup Long Mode
        boot::setup_long_mode(&vcpu_fd, 0x10000, 0x15000)?;

        // 7. Set Registers
        let mut regs = vcpu_fd.get_regs().expect("Failed to get regs");
        regs.rflags = 2;
        regs.rip = kernel_loader_result.kernel_load.0;
        regs.rsp = 0x90_000; // Set a valid stack pointer
        regs.rsi = 0x7_000; // Zero Page Address (LinuxBootConfigurator default)
        vcpu_fd.set_regs(&regs).expect("Failed to set regs");

        println!("Kernel loaded at {:#x}", regs.rip);

        // 8. Run Loop
        println!("Starting VM...");
        let start_time = std::time::Instant::now();
        loop {
            if start_time.elapsed() > std::time::Duration::from_secs(10) {
                println!("VM execution timed out after 10 seconds");
                break;
            }

            match vcpu_fd.run() {
                Ok(VcpuExit::MmioRead(addr, data)) => {
                    if (0xd000_0000..0xd000_1000).contains(&addr) {
                        mmio_transport.read(addr - 0xd000_0000, data);
                    }
                }
                Ok(VcpuExit::MmioWrite(addr, data)) => {
                    if (0xd000_0000..0xd000_1000).contains(&addr) {
                        mmio_transport.write(addr - 0xd000_0000, data);
                        if mmio_transport.get_interrupt_status() != 0 {
                            let _irq_line_result = self.vm_fd.set_irq_line(5, true);
                        } else {
                            let _irq_line_result = self.vm_fd.set_irq_line(5, false);
                        }
                    }
                }
                Ok(VcpuExit::IoIn(addr, data)) => {
                    match addr {
                        0x3fd => {
                            // LSR (Line Status Register)
                            // Bit 5: THRE (Transmitter Holding Register Empty)
                            data[0] = 0x20;
                        }
                        0x61 => {
                            // Keyboard Controller Port B (Speaker/NMI)
                            // Just return 0 for now
                            data[0] = 0;
                        }
                        0x3f8..=0x3ff => {
                            // Serial Port Registers (ignore read for now, except LSR)
                            if addr == 0x3fd {
                                data[0] = 0x20; // THRE
                            }
                        }
                        _ => {
                            println!("IO In at {addr:#x}");
                        }
                    }
                }
                Ok(VcpuExit::IoOut(addr, data)) => {
                    if addr == 0x3f8 {
                        // Serial Port Data Register
                        if let Ok(s) = std::str::from_utf8(data) {
                            print!("{s}");
                        } else {
                            print!("{data:?}");
                        }
                        let _flush_result = std::io::stdout().flush();
                    } else if addr == 0x80 {
                        // IO Delay port, ignore
                    } else if (0x3f8..=0x3ff).contains(&addr) {
                        // Ignore other serial port writes
                    } else {
                        println!("IO Out at {addr:#x}");
                    }
                }
                Ok(VcpuExit::Hlt) => {
                    println!("VM Halted");
                    break;
                }
                Ok(VcpuExit::Shutdown) => {
                    println!("VM Shutdown");
                    break;
                }
                Ok(VcpuExit::FailEntry(reason, qual)) => {
                    println!("VM Fail Entry: reason={reason}, qual={qual}");
                    break;
                }
                Ok(VcpuExit::InternalError) => {
                    println!("VM Internal Error");
                    break;
                }
                Ok(exit_reason) => {
                    println!("Exit: {exit_reason:?}");
                }
                Err(e) => {
                    if e.errno() == libc::EINTR {
                        continue;
                    }
                    panic!("VCPU run failed: {e}");
                }
            }
        }

        Ok(())
    }

    fn setup_gdt(mem: &GuestMemoryMmap, gdt_addr: GuestAddress) -> std::io::Result<()> {
        // GDT Layout:
        // 0x00: Null
        // 0x08: Code (Long Mode)
        // 0x10: Data

        let gdt_table = [
            0x0000_0000_0000_0000_u64, // Null
            0x00af_9a00_0000_ffff_u64, // Code: P=1, L=1, D=0, Type=0xA (Exec/Read), Base=0, Limit=0xFFFFF (ignored)
            0x00cf_9200_0000_ffff_u64, // Data: P=1, L=0, D=1, Type=0x2 (Read/Write), Base=0, Limit=0xFFFFF
        ];

        for (i, entry) in gdt_table.iter().enumerate() {
            mem.write_obj(*entry, GuestAddress(gdt_addr.0 + i as u64 * 8))
                .map_err(std::io::Error::other)?;
        }

        Ok(())
    }

    fn setup_page_tables(mem: &GuestMemoryMmap, pml4_addr: GuestAddress) -> std::io::Result<()> {
        // Simple identity mapping for the first 4GB
        // PML4 -> PDP -> PD -> 2MB Pages

        let pml4_offset = pml4_addr.0;
        let pdp_offset = pml4_offset + 0x1000;
        let pd_offset = pdp_offset + 0x1000;

        // Flags
        let flags = 0x3; // Present | RW

        // PML4[0] -> PDP
        mem.write_obj(pdp_offset | flags, pml4_addr)
            .map_err(std::io::Error::other)?;

        // PDP[0] -> PD
        mem.write_obj(pd_offset | flags, GuestAddress(pdp_offset))
            .map_err(std::io::Error::other)?;

        // PD[0..512] -> 2MB Pages
        for i in 0u64..512 {
            let entry = (i * 0x0020_0000) | flags | 0x80; // Present | RW | PS (2MB)
            mem.write_obj(entry, GuestAddress(pd_offset + i * 8))
                .map_err(std::io::Error::other)?;
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    #[ignore = "requires /dev/kvm (not available on most CI runners); run manually with `cargo test -p locald-vmm -- --ignored`"]
    fn test_boot_kernel() {
        let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let assets_dir = manifest_dir.join("assets");

        // Ensure assets are present
        let (kernel, _) = fetch_kernel::ensure_assets(&assets_dir).expect("Failed to fetch assets");

        let mut vm = VirtualMachine::new();
        // 128MB RAM
        vm.run_kernel(&kernel, 128).expect("Failed to run kernel");
    }
}
