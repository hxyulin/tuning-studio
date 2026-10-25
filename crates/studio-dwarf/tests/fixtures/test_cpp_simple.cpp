/*
 * Simplified C++ test source with classes, namespaces, templates
 * Avoids virtual functions and complex initialization for easier linking
 */

#include <cstdint>

// Namespace test
namespace sensors {
    struct Temperature {
        float celsius;
        float fahrenheit;
    };

    // Nested namespace
    namespace internal {
        uint32_t calibration_value = 1000;
    }
}

// Simple POD class
class Point2D {
public:
    int32_t x;
    int32_t y;
};

// Class with inheritance (no virtual functions)
struct Base {
    uint32_t base_id;
};

struct Derived : public Base {
    float derived_value;
};

// Template class (will be instantiated)
template<typename T>
struct Container {
    T data;
    uint32_t size;
};

// Pointer types
struct Node {
    int32_t value;
    Node* next;
};

// Reference type
struct RefHolder {
    int32_t* ptr;
    int32_t value;
};

// Global instances
volatile sensors::Temperature temp_sensor = {25.0f, 77.0f};
volatile Point2D point = {0, 0};
volatile Derived derived_obj = {{1}, 3.14f};
volatile Container<uint32_t> int_container = {0, 0};
volatile Container<float> float_container = {0.0f, 0};
volatile Node list_head = {0, nullptr};
volatile RefHolder ref_holder = {nullptr, 42};

// Pointers to complex types
volatile sensors::Temperature* temp_ptr = const_cast<sensors::Temperature*>(&temp_sensor);
volatile Node* node_ptr = const_cast<Node*>(&list_head);

// Const data
const float PI = 3.14159f;
const char MESSAGE[] = "Test";

int main(void) {
    while (1) {
        temp_sensor.celsius += 1.0f;
        point.x++;
        int_container.size++;
        list_head.value++;
    }
    return 0;
}
