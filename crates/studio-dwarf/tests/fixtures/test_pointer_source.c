/*
 * Test source with pointer types for ARM ELF fixture
 */

#include <stdint.h>

// Target data
volatile uint32_t target_value = 42;
volatile float float_value = 3.14f;

// Pointers
volatile uint32_t* data_ptr = &target_value;
volatile float* float_ptr = &float_value;
volatile uint32_t** double_ptr = &data_ptr;

// Null pointer
volatile uint32_t* null_ptr = 0;

int main(void) {
    while (1) {
        if (data_ptr != 0) {
            (*data_ptr)++;
        }

        if (float_ptr != 0) {
            (*float_ptr) += 0.01f;
        }
    }
    return 0;
}
