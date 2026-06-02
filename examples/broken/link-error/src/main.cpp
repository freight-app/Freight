#include <iostream>

// Declared here, but never defined anywhere in the project.
// The linker will fail with "undefined reference to compute_answer".
extern int compute_answer(int n);

int main() {
    std::cout << "The answer is: " << compute_answer(42) << "\n";
    return 0;
}
