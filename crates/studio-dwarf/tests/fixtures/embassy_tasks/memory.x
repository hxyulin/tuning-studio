MEMORY
{
  FLASH : ORIGIN = 0x08000000, LENGTH = 256K
  RAM   : ORIGIN = 0x20000000, LENGTH = 64K
}

/* rm-task-stats' counters, as the rm-embedded-rs board scripts place them */
SECTIONS
{
  .probe (NOLOAD) :
  {
    KEEP(*(.probe .probe.*));
  } > RAM
} INSERT AFTER .bss;
