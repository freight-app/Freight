CC      = gcc
CFLAGS  = -Wall -std=c17 -DUSE_FOO -Iinclude
LDLIBS  = -lm -lpthread
TARGET  = demo
SRCS    = src/main.c src/util.c

$(TARGET): $(SRCS)
	$(CC) $(CFLAGS) -o $@ $(SRCS) $(LDLIBS)

clean:
	rm -f $(TARGET)
