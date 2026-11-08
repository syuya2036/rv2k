OUTPUT_ARCH(riscv)
ENTRY(_start)
MEMORY {
 FLASH (rx) : ORIGIN = 0, LENGTH = 16K
 RAM (rw) : ORIGIN = 0x20000000, LENGTH = 2K
}
SECTIONS {
 .text : { KEEP(*(.init)) *(.text .text.*) *(.rodata .rodata.*) } > FLASH
 .data : ALIGN(4) { _sdata = .; *(.data .data.* .sdata .sdata.*) . = ALIGN(4); _edata = .; } > RAM AT> FLASH
 _sidata = LOADADDR(.data);
 .bss (NOLOAD) : ALIGN(4) { _sbss = .; *(.bss .bss.* .sbss .sbss.*) *(COMMON) . = ALIGN(4); _ebss = .; } > RAM
 _stack_top = ORIGIN(RAM) + LENGTH(RAM);
 PROVIDE(_kernel_stack_size = 512);
 _stack_bottom = _stack_top - _kernel_stack_size;
 ASSERT(_ebss <= _stack_bottom, "RAM exceeds budget including kernel stack")
 /DISCARD/ : { *(.eh_frame*) }
}
