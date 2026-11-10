package protocol

import (
	"bytes"
	"encoding/base64"
	"image"
	"image/color"
	"image/jpeg"
	"strings"
	"testing"
)

func testCustomAvatar(t *testing.T, width, height int) string {
	t.Helper()
	img := image.NewRGBA(image.Rect(0, 0, width, height))
	for y := 0; y < height; y++ {
		for x := 0; x < width; x++ {
			img.SetRGBA(x, y, color.RGBA{R: uint8(x), G: uint8(y), B: 90, A: 255})
		}
	}
	var data bytes.Buffer
	if err := jpeg.Encode(&data, img, &jpeg.Options{Quality: 70}); err != nil {
		t.Fatal(err)
	}
	return CustomAvatarPrefix + base64.StdEncoding.EncodeToString(data.Bytes())
}

func TestIsSupportedAvatarAcceptsBoundedCustomJPEG(t *testing.T) {
	upload := testCustomAvatar(t, AvatarSide, AvatarSide)
	if !IsSupportedAvatar(upload) {
		t.Fatal("expected valid custom avatar")
	}
	reference, data, err := CanonicalAvatar(upload)
	if err != nil || !strings.HasPrefix(reference, CustomAvatarRefPrefix) || data != upload {
		t.Fatalf("unexpected canonical avatar: %q %v", reference, err)
	}
	if !IsSupportedAvatar(reference) {
		t.Fatal("expected canonical reference to be valid")
	}
}

func TestIsSupportedAvatarRejectsMalformedOrWrongSizeImages(t *testing.T) {
	cases := []string{
		CustomAvatarPrefix + "not-base64",
		testCustomAvatar(t, 127, 128),
		CustomAvatarPrefix + base64.StdEncoding.EncodeToString([]byte("not jpeg")),
		CustomAvatarPrefix + base64.StdEncoding.EncodeToString(bytes.Repeat([]byte{0}, MaxAvatarJPEGBytes+1)),
		CustomAvatarPrefix + strings.Repeat("A", base64.StdEncoding.EncodedLen(MaxAvatarJPEGBytes)+1),
	}
	for _, avatar := range cases {
		if IsSupportedAvatar(avatar) {
			t.Fatalf("unexpected valid avatar of length %d", len(avatar))
		}
	}
}
