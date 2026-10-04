#include "play_sound.h"

#include "decomp/audio/external.h"
#include "debug_print.h"
#include "load_audio_data.h"

SM64PlaySoundFunctionPtr g_play_sound_func = NULL;

// ER Mario: a Mario that ticks without the audio running (the one in the menus) must not queue
// sounds; they'd all start once it runs, the snoring for good, its source being gone by then.
int g_er_mute = 0;

extern void play_sound( uint32_t soundBits, f32 *pos ) {
    if ( g_er_mute ) return;
    if ( g_is_audio_initialized ) {
        DEBUG_PRINT("$ play_sound(%d) request %d; pos %f %f %f\n", soundBits,sSoundRequestCount,pos[0],pos[1],pos[2]);
        sSoundRequests[sSoundRequestCount].soundBits = soundBits;
        sSoundRequests[sSoundRequestCount].position = pos;
        sSoundRequestCount++;
    }

    if ( g_play_sound_func ) {
        g_play_sound_func(soundBits, pos);
    }
}