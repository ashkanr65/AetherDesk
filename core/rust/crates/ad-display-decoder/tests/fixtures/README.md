# Isolated desktop updates

These four small, synthetic H.264 access units were generated with GStreamer
1.28.6 on 29 September 2026. They contain a 64x64 moving test ball, no user data.
Frame 0 is a keyframe; frames 1-3 depend on earlier frames.

Generation pipeline (pull one appsink sample into each file):

```text
videotestsrc num-buffers=4 pattern=ball !
video/x-raw,format=I420,width=64,height=64,framerate=30/1 !
x264enc tune=zerolatency speed-preset=ultrafast bframes=0 byte-stream=true key-int-max=30 !
h264parse ! video/x-h264,stream-format=byte-stream,alignment=au ! appsink sync=false
```

The tests submit each access unit to the production decoder, wait for that
exact frame before submitting another, and repeat after a flush. This catches
the Windows D3D11 one-frame holdback that continuous playback masks. Fixtures
keep the test independent of an encoder plugin or Linux-only agent code.
