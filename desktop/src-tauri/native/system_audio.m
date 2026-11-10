#import <Foundation/Foundation.h>
#import <CoreAudio/CoreAudio.h>
#import <CoreAudio/AudioHardwareTapping.h>
#import <CoreAudio/CATapDescription.h>
#include <pthread.h>
#include <unistd.h>
#include <math.h>
#include <stdbool.h>
#include <string.h>
#include "system_audio.h"

// A bounded PCM queue, never waiting for the frontend on the audio thread.
#define CAPACITY 9600
struct Capture {
    AudioObjectID tap, device;
    AudioDeviceIOProcID proc;
    pthread_mutex_t lock;
    float samples[CAPACITY * 2];
    uint32_t read, count;
    bool paused;
};
static OSStatus read_property(AudioObjectID object, AudioObjectPropertySelector selector, void *value, UInt32 size) {
    AudioObjectPropertyAddress address={selector,kAudioObjectPropertyScopeGlobal,kAudioObjectPropertyElementMain};
    return AudioObjectGetPropertyData(object,&address,0,NULL,&size,value);
}
static OSStatus receive_audio(AudioObjectID device,const AudioTimeStamp *now,const AudioBufferList *input,
    const AudioTimeStamp *when,AudioBufferList *output,const AudioTimeStamp *outTime,void *context) {
    struct Capture *c=context;
    if (!input || !input->mNumberBuffers || pthread_mutex_trylock(&c->lock)) return noErr;
    if(c->paused){pthread_mutex_unlock(&c->lock);return noErr;}
    const AudioBuffer *first=&input->mBuffers[0];
    uint32_t channels=first->mNumberChannels;
    if (!first->mData || !channels) {pthread_mutex_unlock(&c->lock);return noErr;}
    uint32_t frames=first->mDataByteSize/(sizeof(float)*channels);
    const float *left=first->mData,*right=NULL;
    if(channels==1 && input->mNumberBuffers>1){
        const AudioBuffer *second=&input->mBuffers[1];
        if(second->mData && second->mNumberChannels==1){right=second->mData;frames=MIN(frames,second->mDataByteSize/sizeof(float));}
    }
    // Keep the newest audio only; a stalled UI cannot accumulate seconds of delay.
    for(uint32_t i=frames>CAPACITY?frames-CAPACITY:0;i<frames;i++){
        if(c->count==CAPACITY){c->read=(c->read+1)%CAPACITY;c->count--;}
        uint32_t write=(c->read+c->count)%CAPACITY;
        float l=left[i*channels],r=right?right[i]:(channels>1?left[i*channels+1]:l);
        c->samples[write*2]=isfinite(l)?l:0;c->samples[write*2+1]=isfinite(r)?r:0;c->count++;
    }
    pthread_mutex_unlock(&c->lock);return noErr;
}
void eden_audio_stop(void *handle){
    if(!handle)return;
    struct Capture *c=handle;
    if(c->proc){AudioDeviceStop(c->device,c->proc);AudioDeviceDestroyIOProcID(c->device,c->proc);}
    if(c->device)AudioHardwareDestroyAggregateDevice(c->device);
    if(@available(macOS 14.2,*)){if(c->tap)AudioHardwareDestroyProcessTap(c->tap);}
    pthread_mutex_destroy(&c->lock);free(c);
}
int32_t eden_audio_start(void **handle,double *sample_rate){
    *handle=NULL;
    if(@available(macOS 14.2,*)){
        @autoreleasepool {
            // Resolve this exact process rather than excluding unrelated apps.
            pid_t pid=getpid();AudioObjectID own=0;UInt32 size=sizeof(own);
            AudioObjectPropertyAddress address={kAudioHardwarePropertyTranslatePIDToProcessObject,kAudioObjectPropertyScopeGlobal,kAudioObjectPropertyElementMain};
            OSStatus status=AudioObjectGetPropertyData(kAudioObjectSystemObject,&address,sizeof(pid),&pid,&size,&own);
            if(status || !own)return status?:-71001; // Fail closed: never capture our own voice output.
            struct Capture *c=calloc(1,sizeof(*c));if(!c)return -71002;
            pthread_mutex_init(&c->lock,NULL);
            CATapDescription *description=[[CATapDescription alloc] initStereoGlobalTapButExcludeProcesses:@[@(own)]];
            description.name=@"EdenVoice screen audio";description.private=YES;description.muteBehavior=CATapUnmuted;
            status=AudioHardwareCreateProcessTap(description,&c->tap);
            if(status){eden_audio_stop(c);return status;}
            AudioStreamBasicDescription format={0};status=read_property(c->tap,kAudioTapPropertyFormat,&format,sizeof(format));
            if(status || format.mFormatID!=kAudioFormatLinearPCM || !(format.mFormatFlags&kAudioFormatFlagIsFloat) || format.mBitsPerChannel!=32 || format.mChannelsPerFrame!=2 || format.mSampleRate<8000 || format.mSampleRate>192000){eden_audio_stop(c);return status?:-71003;}
            NSDictionary *aggregate=@{
                @kAudioAggregateDeviceNameKey:@"EdenVoice screen audio",
                @kAudioAggregateDeviceUIDKey:NSUUID.UUID.UUIDString,
                @kAudioAggregateDeviceIsPrivateKey:@YES,
                @kAudioAggregateDeviceTapAutoStartKey:@YES,
                @kAudioAggregateDeviceTapListKey:@[@{@kAudioSubTapUIDKey:description.UUID.UUIDString,@kAudioSubTapDriftCompensationKey:@YES}]
            };
            status=AudioHardwareCreateAggregateDevice((__bridge CFDictionaryRef)aggregate,&c->device);
            if(!status)status=AudioDeviceCreateIOProcID(c->device,receive_audio,c,&c->proc);
            if(!status)status=AudioDeviceStart(c->device,c->proc);
            if(status){eden_audio_stop(c);return status;}
            *sample_rate=format.mSampleRate;*handle=c;return 0;
        }
    }
    return -71004;
}
uint32_t eden_audio_read(void *handle,float *stereo,uint32_t max_frames){
    if(!handle)return 0;struct Capture *c=handle;
    pthread_mutex_lock(&c->lock);uint32_t frames=MIN(c->count,max_frames);
    for(uint32_t i=0;i<frames;i++){uint32_t p=(c->read+i)%CAPACITY;stereo[i*2]=c->samples[p*2];stereo[i*2+1]=c->samples[p*2+1];}
    c->read=(c->read+frames)%CAPACITY;c->count-=frames;pthread_mutex_unlock(&c->lock);return frames;
}

void eden_audio_set_paused(void *handle,int32_t paused){
    if(!handle)return;struct Capture *c=handle;
    pthread_mutex_lock(&c->lock);c->paused=paused!=0;c->read=0;c->count=0;
    memset(c->samples,0,sizeof(c->samples));pthread_mutex_unlock(&c->lock);
}
