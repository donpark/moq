package moq

import (
	"math"
	"testing"
	"time"
)

// A peer's delay or max age can pass time.Duration's ~292 years, which must read as
// the longest duration rather than wrap to a negative one that is already expired.
func TestFromMicrosSaturates(t *testing.T) {
	limit := uint64(math.MaxInt64 / int64(time.Microsecond))
	cases := []struct {
		us   uint64
		want time.Duration
	}{
		{us: 1, want: time.Microsecond},
		{us: limit, want: time.Duration(limit) * time.Microsecond},
		{us: limit + 1, want: time.Duration(math.MaxInt64)},
		{us: math.MaxUint64, want: time.Duration(math.MaxInt64)},
	}
	for _, c := range cases {
		if got := fromMicros(c.us); got != c.want {
			t.Errorf("fromMicros(%d) = %v, want %v", c.us, got, c.want)
		}
	}
}

// The zero Subscription skips superseded groups at once, as moq-net's default does, and
// only NoMaxDelay asks for no subscriber limit.
func TestSubscriptionMaxDelay(t *testing.T) {
	cases := []struct {
		name string
		in   time.Duration
		want *time.Duration
	}{
		{name: "zero value", in: 0, want: new(time.Duration)},
		{name: "budget", in: time.Second, want: ptr(time.Second)},
		{name: "no limit", in: NoMaxDelay, want: nil},
	}
	for _, c := range cases {
		got, err := Subscription{MaxDelay: c.in}.ffi()
		if err != nil {
			t.Fatalf("%s: %v", c.name, err)
		}
		if (got.MaxDelay == nil) != (c.want == nil) || (got.MaxDelay != nil && *got.MaxDelay != *c.want) {
			t.Errorf("%s: MaxDelay = %v, want %v", c.name, got.MaxDelay, c.want)
		}
	}
	if _, err := (Subscription{MaxDelay: -time.Millisecond}).ffi(); err == nil {
		t.Error("a negative max delay was accepted, want an error")
	}
}
