#import <Foundation/Foundation.h>

#include <algorithm>
#include <numeric>
#include <string>
#include <utility>
#include <vector>

@interface ScoreReport : NSObject {
    std::vector<int> _scores;
}
- (instancetype)initWithScores:(std::vector<int>)scores;
- (double)average;
- (NSString *)summary;
@end

@implementation ScoreReport
- (instancetype)initWithScores:(std::vector<int>)scores {
    self = [super init];
    if (self) {
        _scores = std::move(scores);
    }
    return self;
}

- (double)average {
    if (_scores.empty()) {
        return 0.0;
    }
    int total = std::accumulate(_scores.begin(), _scores.end(), 0);
    return static_cast<double>(total) / static_cast<double>(_scores.size());
}

- (NSString *)summary {
    std::vector<int> sorted = _scores;
    std::sort(sorted.begin(), sorted.end());

    std::string joined;
    for (int value : sorted) {
        if (!joined.empty()) {
            joined += ", ";
        }
        joined += std::to_string(value);
    }

    return [NSString stringWithFormat:@"scores=[%s], average=%.2f",
                                      joined.c_str(),
                                      [self average]];
}
@end

int main(void) {
    @autoreleasepool {
        ScoreReport *report =
            [[ScoreReport alloc] initWithScores:std::vector<int>{91, 84, 100, 73, 88}];
        NSLog(@"objcpp-hello: %@", [report summary]);
    }

    return 0;
}
