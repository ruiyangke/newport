/* Reproduces the production pull crash: printf calls strnlen(s, INT_MAX)
 * for a valid string whose terminating NUL is near an unmapped page.
 * Test the cross compiler's actual libc, not the host test runner's libc. */
#include <limits.h>
#include <stdio.h>
#include <string.h>
#include <sys/mman.h>
#include <unistd.h>

int main(void) {
    size_t page = (size_t)sysconf(_SC_PAGESIZE);
    char *memory = mmap(NULL, page * 2, PROT_READ | PROT_WRITE,
                        MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    if (memory == MAP_FAILED || mprotect(memory + page, page, PROT_NONE)) return 1;
    size_t (*volatile length)(const char *, size_t) = strnlen;
    for (size_t offset = 1; offset <= 128; ++offset) {
        for (size_t trailing = 0; trailing < offset; ++trailing) {
            char *string = memory + page - offset;
            size_t expected = offset - trailing - 1;
            memset(string, 'x', offset);
            string[expected] = '\0';
            if (length(string, INT_MAX) != expected) return 2;
            if (length(string, expected / 2) != expected / 2) return 3;
            char output[129];
            if (snprintf(output, sizeof(output), "%s", string) != (int)expected) return 4;
            if (memcmp(output, string, expected + 1)) return 5;
        }
    }
    if (munmap(memory, page * 2)) return 6;
    puts("Runtime string boundary regression passed (8,256 layouts)");
    return 0;
}
