// Shared C language parameters — standards only.
// Extensions differ per compiler (gcc includes .s/.S, clang does not).
// linking["c"] differs per compiler (compile_binary varies).

defaults["std"] = "c11";

standards["c11"] = "-std=c11";
standards["c17"] = "-std=c17";
standards["c23"] = "-std=c23";
