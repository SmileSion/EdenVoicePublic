// Pure callback/ring-buffer test: no real device, microphone, or OS permission.
// clang -fobjc-arc -framework Foundation -framework CoreAudio scripts/system-audio-privacy.test.m -o /tmp/eden-audio-privacy-test && /tmp/eden-audio-privacy-test
#include <assert.h>
#import "../src-tauri/native/system_audio.m"
int main(void) {
    struct Capture capture={0};pthread_mutex_init(&capture.lock,NULL);
    float samples[8]={.4f,-.4f,.3f,-.3f,.2f,-.2f,.1f,-.1f},out[16]={0};
    AudioBufferList input={.mNumberBuffers=1,.mBuffers={{.mNumberChannels=2,.mDataByteSize=sizeof(samples),.mData=samples}}};
    receive_audio(0,NULL,&input,NULL,NULL,NULL,&capture);assert(capture.count==4);
    eden_audio_set_paused(&capture,1);assert(capture.paused&&capture.count==0);
    for(unsigned i=0;i<CAPACITY*2;i++)assert(capture.samples[i]==0);
    receive_audio(0,NULL,&input,NULL,NULL,NULL,&capture);
    assert(eden_audio_read(&capture,out,8)==0); // paused samples never enter queue
    eden_audio_set_paused(&capture,0);assert(!capture.paused);
    assert(eden_audio_read(&capture,out,8)==0); // old audio cannot replay after resume
    receive_audio(0,NULL,&input,NULL,NULL,NULL,&capture);
    assert(eden_audio_read(&capture,out,8)==4);assert(out[0]==.4f&&out[1]==-.4f);
    pthread_mutex_destroy(&capture.lock);puts("PASS: native screen audio gate, queue purge, resume fresh samples");return 0;
}
