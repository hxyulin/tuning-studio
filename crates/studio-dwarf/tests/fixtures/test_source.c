/*
 * Simple test source for ARM ELF fixture
 * Contains basic variable types for testing DWARF parsing
 */

#include <stdint.h>

// Simple global variables
volatile uint32_t global_counter = 0;
volatile float sensor_data = 0.0f;
volatile int8_t status_flag = 0;
volatile uint16_t sample_rate = 1000;

// Enum type
typedef enum {
    STATE_IDLE = 0,
    STATE_RUNNING = 1,
    STATE_ERROR = 2
} SystemState;

volatile SystemState current_state = STATE_IDLE;

// Main function (required for linking)
int main(void) {
    while (1) {
        global_counter++;
        sensor_data += 0.1f;

        if (global_counter % 100 == 0) {
            status_flag = 1;
        }
    }
    return 0;
}
