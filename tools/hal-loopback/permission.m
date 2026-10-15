/* Read-only preflight. The user controls microphone permission in macOS. */
#import <AVFoundation/AVFoundation.h>
int ftu_audio_authorization(void) {
    @autoreleasepool {
        return (int)[AVCaptureDevice authorizationStatusForMediaType:AVMediaTypeAudio];
    }
}
