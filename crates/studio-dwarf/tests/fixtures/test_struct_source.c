/*
 * Test source with struct types for ARM ELF fixture
 */

#include <stdint.h>

// Struct definition
typedef struct {
    uint32_t x;
    uint32_t y;
    float value;
} SensorData;

// Nested struct
typedef struct {
    uint32_t id;
    SensorData sensor;
    uint8_t enabled;
} DeviceConfig;

// Global struct instances
volatile SensorData sensor_struct = {0, 0, 0.0f};
volatile DeviceConfig device_config = {1, {0, 0, 0.0f}, 1};

// Array
volatile uint32_t buffer[8] = {0};

int main(void) {
    while (1) {
        sensor_struct.x++;
        sensor_struct.y++;
        sensor_struct.value += 1.0f;

        buffer[0] = sensor_struct.x;
    }
    return 0;
}
