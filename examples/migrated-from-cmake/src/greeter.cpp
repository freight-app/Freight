#include "greeter.hpp"

std::string greet(std::string_view who) {
    return std::string("Hello, ") + std::string(who) + "!";
}
