use kvm_bindings::{kvm_segment, kvm_sregs};
use kvm_ioctls::VcpuFd;

// Constants for Long Mode setup
const CR0_PE: u64 = 1 << 0;
const CR0_PG: u64 = 1 << 31;
const CR4_PAE: u64 = 1 << 5;
const EFER_LME: u64 = 1 << 8;
const EFER_LMA: u64 = 1 << 10;

// Page Table Entry flags
// const PTE_PRESENT: u64 = 1 << 0;
// const PTE_RW: u64 = 1 << 1;
// const PTE_USER: u64 = 1 << 2;
// const PTE_ACCESSED: u64 = 1 << 5;
// const PTE_DIRTY: u64 = 1 << 6;
// const PTE_PSE: u64 = 1 << 7;

pub fn setup_long_mode(vcpu: &VcpuFd, page_table_addr: u64, gdt_addr: u64) -> std::io::Result<()> {
    let mut sregs = vcpu.get_sregs()?;

    setup_segments(&mut sregs);

    // Setup GDT in sregs
    sregs.gdt.base = gdt_addr;
    sregs.gdt.limit = 23; // 3 entries * 8 bytes - 1

    // Enable Long Mode
    sregs.cr3 = page_table_addr;
    sregs.cr4 |= CR4_PAE;
    sregs.cr0 |= CR0_PE | CR0_PG;
    sregs.efer |= EFER_LME | EFER_LMA;

    vcpu.set_sregs(&sregs)?;
    Ok(())
}

const fn setup_segments(sregs: &mut kvm_sregs) {
    let code_seg = kvm_segment {
        base: 0,
        limit: 0xffff_ffff,
        selector: 0x8,
        type_: 0xa, // Code, Execute/Read
        present: 1,
        dpl: 0,
        db: 0,
        s: 1, // Code/Data
        l: 1, // Long Mode
        g: 1, // 4KB Granularity
        avl: 0,
        unusable: 0,
        padding: 0,
    };

    let data_seg = kvm_segment {
        base: 0,
        limit: 0xffff_ffff,
        selector: 0x10,
        type_: 0x2, // Data, Read/Write
        present: 1,
        dpl: 0,
        db: 1,
        s: 1,
        l: 0,
        g: 1,
        avl: 0,
        unusable: 0,
        padding: 0,
    };

    sregs.cs = code_seg;
    sregs.ds = data_seg;
    sregs.es = data_seg;
    sregs.fs = data_seg;
    sregs.gs = data_seg;
    sregs.ss = data_seg;
}
