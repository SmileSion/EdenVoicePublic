// Records only RMS/count diagnostics, never captured audio. Requires macOS permission.
#import <Foundation/Foundation.h>
#import <AVFoundation/AVFoundation.h>
#include "../src-tauri/native/system_audio.h"
#include <math.h>
static NSDictionary *measure(void *capture){
    float samples[4096];double squares=0;uint64_t count=0;
    for(int i=0;i<100;i++){
        [NSRunLoop.currentRunLoop runUntilDate:[NSDate dateWithTimeIntervalSinceNow:.02]];
        uint32_t n=eden_audio_read(capture,samples,2048)*2;
        for(uint32_t j=0;j<n;j++)squares+=(double)samples[j]*samples[j];count+=n;
    }
    return @{ @"samples":@(count),@"rms":@(count?sqrt(squares/count):0) };
}
int main(int argc,const char **argv){@autoreleasepool{
    NSString *result=@"/tmp/eden-native-audio-result.json";
    NSError *error=nil;
    AVAudioPlayer *player=[[AVAudioPlayer alloc] initWithContentsOfURL:[NSURL fileURLWithPath:@"/tmp/eden-test-tone.wav"] error:&error];
    player.numberOfLoops=-1;player.volume=.3;[player play];
    [NSRunLoop.currentRunLoop runUntilDate:[NSDate dateWithTimeIntervalSinceNow:.3]];
    void *capture=NULL;double rate=0;int32_t status=eden_audio_start(&capture,&rate);
    NSMutableDictionary *report=[@{@"startStatus":@(status),@"sampleRate":@(rate),@"playerPlaying":@(player.playing),@"playerDuration":@(player.duration),@"playerError":error.localizedDescription?:@""} mutableCopy];
    if(capture){
        report[@"selfOutput"]=measure(capture);[player stop];
        NSTask *external=[NSTask new];external.executableURL=[NSURL fileURLWithPath:@"/usr/bin/afplay"];
        external.arguments=@[@"/tmp/eden-test-tone.wav"];
        [external launchAndReturnError:&error];
        report[@"externalOutput"]=measure(capture);
        if(external.running)[external terminate];[external waitUntilExit];
        eden_audio_stop(capture);report[@"stopped"]=@YES;
    }
    [player stop];
    NSData *data=[NSJSONSerialization dataWithJSONObject:report options:NSJSONWritingPrettyPrinted error:nil];
    [data writeToFile:result atomically:YES];return status?1:0;
}}
