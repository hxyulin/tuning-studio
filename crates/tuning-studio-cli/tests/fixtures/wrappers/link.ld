ENTRY(_start)
MEMORY { FLASH (rx) : ORIGIN = 0x08000000, LENGTH = 512K
RAM (rwx) : ORIGIN = 0x20000000, LENGTH = 128K }
SECTIONS {
.text : { *(.text*) } > FLASH
.rodata : { KEEP(*(.rodata*)) } > FLASH
.data : { KEEP(*(.data*)) } > RAM
.bss (NOLOAD) : { KEEP(*(.bss*)) } > RAM
/DISCARD/ : { *(.ARM.exidx*) *(.ARM.extab*) }
}
