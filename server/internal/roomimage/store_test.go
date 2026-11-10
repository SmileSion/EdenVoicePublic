package roomimage

import (
	"bytes"
	"errors"
	"image"
	"image/jpeg"
	"os"
	"testing"
	"time"

	"edenvoice/server/internal/protocol"
)

func testJPEG(t *testing.T, width, height int) []byte {
	t.Helper()
	var b bytes.Buffer
	if err := jpeg.Encode(&b, image.NewRGBA(image.Rect(0, 0, width, height)), nil); err != nil {
		t.Fatal(err)
	}
	return b.Bytes()
}
func TestImagesPersistSeparatelyAndRequireRoom(t *testing.T) {
	dir := t.TempDir()
	store, err := Open(dir)
	if err != nil {
		t.Fatal(err)
	}
	now := time.Now()
	data := testJPEG(t, 128, 96)
	saved, err := store.Put(1, data, 128, 96, now)
	if err != nil {
		t.Fatal(err)
	}
	reopened, err := Open(dir)
	if err != nil {
		t.Fatal(err)
	}
	meta, got, eof, err := reopened.Get(1, saved.Reference, 0, now)
	if err != nil || !bytes.Equal(got, data) || !eof || meta != saved {
		t.Fatalf("restart did not recover image: %+v %v", meta, err)
	}
	if _, _, _, err = reopened.Get(2, saved.Reference, 0, now); !errors.Is(err, ErrUnavailable) {
		t.Fatal("cross-room image read allowed")
	}
	if _, _, _, err = reopened.Get(1, "../../private", 0, now); !errors.Is(err, ErrUnavailable) {
		t.Fatal("path traversal allowed")
	}
	for _, offset := range []int{-1, len(data), 1 << 30} {
		if _, _, _, err = reopened.Get(1, saved.Reference, offset, now); err == nil {
			t.Fatal("invalid offset accepted")
		}
	}
	files, _ := os.ReadDir(dir)
	if len(files) != 2 {
		t.Fatalf("unexpected files %d", len(files))
	}
	for _, f := range files {
		info, _ := f.Info()
		if info.Mode().Perm() != 0600 {
			t.Fatal("image file not private")
		}
	}
	reopened.Cleanup(now.Add(Retention + time.Second))
	if _, _, _, err = reopened.Get(1, saved.Reference, 0, now.Add(Retention+time.Second)); err == nil {
		t.Fatal("expired image was served")
	}
	files, _ = os.ReadDir(dir)
	if len(files) != 0 {
		t.Fatal("expired image files retained")
	}
}
func TestRejectsMalformedOrOversizedImagesAndBoundsDisk(t *testing.T) {
	data := testJPEG(t, 64, 48)
	for _, tc := range []struct {
		data []byte
		w, h int
	}{{data, 0, 48}, {data, 1601, 48}, {data, 64, 47}, {data[:len(data)/2], 64, 48}, {[]byte("not JPEG"), 64, 48}, {make([]byte, protocol.MaxRoomImageBytes+1), 64, 48}, {data, 1 << 30, 1 << 30}} {
		if Validate(tc.data, tc.w, tc.h) == nil {
			t.Fatalf("invalid image accepted: %d x %d (%d)", tc.w, tc.h, len(tc.data))
		}
	}
	store, err := Open(t.TempDir())
	if err != nil {
		t.Fatal(err)
	}
	store.bytes = MaxDiskBytes
	if _, err = store.Put(1, data, 64, 48, time.Now()); !errors.Is(err, ErrFull) {
		t.Fatalf("disk cap not enforced: %v", err)
	}
	store.bytes = 0
	_ = os.Remove(store.dir)
	if _, err = store.Put(1, data, 64, 48, time.Now()); err == nil {
		t.Fatal("failed persistence reported success")
	}
	if len(store.entries) != 0 {
		t.Fatal("failed image published")
	}
}
func TestChunksBoundedAndTruncationRejected(t *testing.T) {
	store, err := Open(t.TempDir())
	if err != nil {
		t.Fatal(err)
	}
	now := time.Now()
	data := testJPEG(t, 1600, 1600)
	saved, err := store.Put(1, data, 1600, 1600, now)
	if err != nil {
		t.Fatal(err)
	}
	var got []byte
	for offset := 0; offset < len(data); {
		_, chunk, eof, err := store.Get(1, saved.Reference, offset, now)
		if err != nil {
			t.Fatal(err)
		}
		if len(chunk) > 8192 || len(chunk) == 0 {
			t.Fatal("unbounded chunk")
		}
		got = append(got, chunk...)
		offset += len(chunk)
		if eof != (offset == len(data)) {
			t.Fatal("bad eof")
		}
	}
	if !bytes.Equal(got, data) {
		t.Fatal("chunk data corrupted")
	}
	if err = os.Truncate(store.path(saved.Reference, ".jpg"), 1); err != nil {
		t.Fatal(err)
	}
	if _, _, _, err = store.Get(1, saved.Reference, 0, now); err == nil {
		t.Fatal("truncated disk data returned as valid image")
	}
}
