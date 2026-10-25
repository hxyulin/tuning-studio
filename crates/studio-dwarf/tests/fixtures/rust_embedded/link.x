MEMORY
{
  FLASH : ORIGIN = 0x08000000, LENGTH = 256K
  RAM   : ORIGIN = 0x20000000, LENGTH = 64K
}

ENTRY(reset);

SECTIONS
{
  .vector_table ORIGIN(FLASH) : { LONG(ORIGIN(RAM) + LENGTH(RAM)); KEEP(*(.vector_table.reset)); } > FLASH
  .text : { *(.text .text.*); } > FLASH
  .rodata : { *(.rodata .rodata.*); } > FLASH
  .data : { *(.data .data.*); } > RAM AT > FLASH
  .bss (NOLOAD) : { *(.bss .bss.*); } > RAM
  /DISCARD/ : { *(.ARM.exidx .ARM.exidx.*); }
}
