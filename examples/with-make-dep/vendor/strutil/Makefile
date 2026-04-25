CC     = gcc
AR     = ar
CFLAGS = -Iinclude -Wall -O2

SRCS = src/strutil.c
OBJS = $(SRCS:.c=.o)
LIB  = libstrutil.a

.PHONY: all clean

all: $(LIB)

$(LIB): $(OBJS)
	$(AR) rcs $@ $^

%.o: %.c
	$(CC) $(CFLAGS) -c $< -o $@

clean:
	rm -f $(OBJS) $(LIB)
