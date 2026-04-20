MEMORY
{
    FLASH     : ORIGIN = 0x08000000, LENGTH = 4096K /* BANK_1 + BANK_2 */
    RAM       : ORIGIN = 0x20000000, LENGTH = 3008K /* SRAM + SRAM2 + SRAM3 + SRAM5 + SRAM6 */
    NOR_FLASH : ORIGIN = 0xA0000000, LENGTH = 32M
    BKPSRAM   : ORIGIN = 0x40036400, LENGTH = 1K 
    DEFMT_PERSIST : ORIGIN = 0x40036800, LENGTH = 1K
}

__defmt_persist_start = ORIGIN(DEFMT_PERSIST);
__defmt_persist_end   = ORIGIN(DEFMT_PERSIST) + LENGTH(DEFMT_PERSIST);

SECTIONS
{
  .image_buf (NOLOAD) : ALIGN(8) {
    *(.image_buf .image_buf.*);
    . = ALIGN(8);
    } > RAM
}