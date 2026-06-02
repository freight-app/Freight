#import <Foundation/Foundation.h>

@interface RunningTotal : NSObject
@property(nonatomic) NSInteger value;
- (instancetype)initWithValue:(NSInteger)value;
- (void)add:(NSInteger)delta;
@end

@implementation RunningTotal
- (instancetype)initWithValue:(NSInteger)value {
    self = [super init];
    if (self) {
        _value = value;
    }
    return self;
}

- (void)add:(NSInteger)delta {
    _value += delta;
}
@end

int main(void) {
    @autoreleasepool {
        NSArray<NSNumber *> *samples = @[ @3, @5, @8, @13, @21 ];
        RunningTotal *total = [[RunningTotal alloc] initWithValue:0];

        for (NSNumber *sample in samples) {
            [total add:sample.integerValue];
        }

        NSLog(@"objc-hello: %@ -> total=%ld", samples, (long)total.value);
    }

    return 0;
}
