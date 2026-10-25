/*
 * Complex C test source with packed structs, bitfields, function pointers
 */

#include <stdint.h>

// Packed struct
typedef struct __attribute__((packed)) {
    uint8_t flags;
    uint32_t timestamp;
    uint16_t value;
} PackedData;

// Struct with bitfields
typedef struct {
    uint32_t flag1 : 1;
    uint32_t flag2 : 1;
    uint32_t counter : 6;
    uint32_t reserved : 24;
} BitfieldStruct;

// Union
typedef union {
    uint32_t as_uint32;
    float as_float;
    struct {
        uint16_t low;
        uint16_t high;
    } as_words;
} DataUnion;

// Function pointer type
typedef void (*callback_t)(uint32_t);

// Struct with function pointer
typedef struct {
    uint32_t id;
    callback_t handler;
    void* user_data;
} EventHandler;

// Anonymous struct (C11)
struct {
    uint32_t x;
    uint32_t y;
} anonymous_struct;

// Deeply nested struct
typedef struct {
    struct {
        struct {
            uint32_t inner_value;
        } level2;
    } level1;
} NestedStruct;

// Array of structs
typedef struct {
    uint32_t sensor_id;
    float value;
} Measurement;

// Flexible array member (C99)
typedef struct {
    uint32_t count;
    uint32_t data[];
} FlexibleArray;

// Global instances
volatile PackedData packed_data = {0, 0, 0};
volatile BitfieldStruct bitfield_data = {0, 0, 0, 0};
volatile DataUnion data_union = {0};
volatile EventHandler event_handler = {1, 0, 0};
volatile NestedStruct nested = {{{42}}};
volatile Measurement measurements[4] = {{0, 0.0f}};

// Const data
const uint32_t MAGIC_NUMBER = 0xDEADBEEF;
const char* const MESSAGE = "Test";

// Static variable (should be optimized out or marked as local)
static uint32_t internal_counter = 0;

// Extern variable (should be marked as extern)
extern uint32_t external_value;

int main(void) {
    while (1) {
        packed_data.value++;
        bitfield_data.counter++;
        data_union.as_uint32++;
        nested.level1.level2.inner_value++;
        internal_counter++;
    }
    return 0;
}
