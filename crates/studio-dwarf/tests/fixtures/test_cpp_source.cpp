/*
 * C++ test source with classes, namespaces, references, templates
 */

#include <cstdint>

// Namespace test
namespace sensors {
    class Temperature {
    public:
        float celsius;
        float fahrenheit;

        Temperature() : celsius(0.0f), fahrenheit(32.0f) {}

        void update(float c) {
            celsius = c;
            fahrenheit = c * 9.0f / 5.0f + 32.0f;
        }
    };

    // Nested namespace (C++17)
    namespace internal {
        uint32_t calibration_value = 1000;
    }
}

// Class with inheritance
class Base {
public:
    uint32_t base_id;
    virtual ~Base() {}
};

class Derived : public Base {
public:
    float derived_value;
};

// Template class
template<typename T>
class Container {
public:
    T data;
    uint32_t size;

    Container() : size(0) {}
};

// Complex pointer types
class Node {
public:
    int32_t value;
    Node* next;
    Node** indirect;

    Node() : value(0), next(nullptr), indirect(nullptr) {}
};

// References
class RefTest {
public:
    int32_t& ref_member;
    const int32_t& const_ref;

    RefTest(int32_t& val) : ref_member(val), const_ref(val) {}
};

// Global instances (zero-initialized to avoid constructors)
volatile sensors::Temperature temp_sensor = {};
volatile Derived derived_obj = {};
volatile Container<uint32_t> int_container = {};
volatile Container<float> float_container = {};
volatile Node list_head = {};
volatile int32_t ref_target = 42;

// Pointers to complex types
volatile sensors::Temperature* temp_ptr = const_cast<sensors::Temperature*>(&temp_sensor);
volatile Node* node_ptr = const_cast<Node*>(&list_head);

// Volatile qualified complex type
volatile const float PI = 3.14159f;

int main(void) {
    while (1) {
        // Access members directly
        temp_sensor.celsius += 0.1f;
        int_container.size++;
        list_head.value++;
    }
    return 0;
}
