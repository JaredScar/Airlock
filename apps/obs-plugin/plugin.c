/* OBS audio filter. Links libobs, so this file is GPL-2.0-or-later.
   It holds one source's audio and can silence that hold. Speech recognition,
   the judge, and RTMP stay in the Airlock app.
   Build with clang --target=x86_64-pc-windows-msvc -shared -D_MSC_VER=1910
   against the OBS 33.0.1 libobs headers and airlock_obs_delay.lib. */

#include <obs-module.h>
#include <util/bmem.h>

#include <stdint.h>

#define WIN32_LEAN_AND_MEAN
#define NOMINMAX
#include <windows.h>

#define MAX_DELAY_MS 60000
#define DEFAULT_DELAY_MS 8000

extern void *airlock_delay_new(uint32_t delay_frames, uint32_t channels);
extern void airlock_delay_free(void *delay);
extern void airlock_delay_set(void *delay, uint32_t delay_frames);
extern void airlock_delay_process(void *delay, float *samples, uint32_t frames);
extern void airlock_delay_dump(void *delay);

struct delay_filter {
	uint32_t delay_ms;
	uint32_t rate;
	uint32_t channels;
	void *delay;
	float *scratch;
	size_t scratch_frames;
	CRITICAL_SECTION lock;
};

static uint32_t clamp_delay_ms(long long ms)
{
	if (ms < 0)
		return 0;
	if (ms > MAX_DELAY_MS)
		return MAX_DELAY_MS;
	return (uint32_t)ms;
}

static uint32_t delay_frames_for(uint32_t delay_ms, uint32_t rate)
{
	uint64_t frames = (uint64_t)delay_ms * (uint64_t)rate / 1000ull;
	uint64_t cap = 60ull * (uint64_t)rate;
	if (frames > cap)
		frames = cap;
	return (uint32_t)frames;
}

static void ensure_delay(struct delay_filter *filter, uint32_t channels, uint32_t rate)
{
	uint32_t frames = delay_frames_for(filter->delay_ms, rate);
	if (filter->delay && filter->channels == channels && filter->rate == rate) {
		airlock_delay_set(filter->delay, frames);
		return;
	}
	if (filter->delay)
		airlock_delay_free(filter->delay);
	filter->channels = channels == 0 ? 1 : channels;
	filter->rate = rate == 0 ? 48000 : rate;
	filter->delay = airlock_delay_new(delay_frames_for(filter->delay_ms, filter->rate), filter->channels);
}

static const char *delay_name(void *type_data)
{
	UNUSED_PARAMETER(type_data);
	return "Airlock Delay";
}

static void delay_defaults(obs_data_t *settings)
{
	obs_data_set_default_int(settings, "delay_ms", DEFAULT_DELAY_MS);
}

static bool dump_clicked(obs_properties_t *props, obs_property_t *property, void *data)
{
	struct delay_filter *filter = data;
	UNUSED_PARAMETER(props);
	UNUSED_PARAMETER(property);
	if (!filter)
		return false;
	EnterCriticalSection(&filter->lock);
	if (filter->delay)
		airlock_delay_dump(filter->delay);
	LeaveCriticalSection(&filter->lock);
	return false;
}

static obs_properties_t *delay_properties(void *data)
{
	obs_properties_t *props = obs_properties_create();
	obs_properties_add_int_slider(props, "delay_ms", "Delay (ms)", 0, MAX_DELAY_MS, 100);
	obs_properties_add_button2(props, "dump", "Dump held audio", dump_clicked, data);
	return props;
}

static void delay_update(void *data, obs_data_t *settings)
{
	struct delay_filter *filter = data;
	uint32_t delay_ms = clamp_delay_ms(obs_data_get_int(settings, "delay_ms"));
	EnterCriticalSection(&filter->lock);
	filter->delay_ms = delay_ms;
	if (filter->delay)
		airlock_delay_set(filter->delay, delay_frames_for(delay_ms, filter->rate == 0 ? 48000 : filter->rate));
	LeaveCriticalSection(&filter->lock);
}

static void *delay_create(obs_data_t *settings, obs_source_t *source)
{
	struct delay_filter *filter = bzalloc(sizeof(*filter));
	UNUSED_PARAMETER(source);
	InitializeCriticalSection(&filter->lock);
	filter->delay_ms = clamp_delay_ms(obs_data_get_int(settings, "delay_ms"));
	filter->rate = 48000;
	filter->channels = 2;
	ensure_delay(filter, 2, 48000);
	return filter;
}

static void delay_destroy(void *data)
{
	struct delay_filter *filter = data;
	if (!filter)
		return;
	EnterCriticalSection(&filter->lock);
	if (filter->delay)
		airlock_delay_free(filter->delay);
	bfree(filter->scratch);
	filter->delay = NULL;
	filter->scratch = NULL;
	LeaveCriticalSection(&filter->lock);
	DeleteCriticalSection(&filter->lock);
	bfree(filter);
}

static struct obs_audio_data *delay_audio(void *data, struct obs_audio_data *audio)
{
	struct delay_filter *filter = data;
	struct obs_audio_info info;
	uint32_t channels = 0;
	uint32_t rate = 48000;
	uint32_t frames;
	uint32_t c;
	uint32_t f;

	if (!filter || !audio)
		return audio;
	frames = audio->frames;
	if (frames == 0)
		return audio;
	for (c = 0; c < MAX_AV_PLANES; c++) {
		if (audio->data[c])
			channels++;
	}
	if (channels == 0)
		return audio;
	if (obs_get_audio_info(&info))
		rate = info.samples_per_sec == 0 ? 48000 : info.samples_per_sec;

	EnterCriticalSection(&filter->lock);
	ensure_delay(filter, channels, rate);
	if (filter->scratch_frames < frames) {
		float *grown = (float *)brealloc(filter->scratch, (size_t)frames * channels * sizeof(float));
		if (!grown) {
			LeaveCriticalSection(&filter->lock);
			return audio;
		}
		filter->scratch = grown;
		filter->scratch_frames = frames;
	}
	for (f = 0; f < frames; f++) {
		for (c = 0; c < channels; c++) {
			float *plane = (float *)audio->data[c];
			filter->scratch[f * channels + c] = plane[f];
		}
	}
	airlock_delay_process(filter->delay, filter->scratch, frames);
	for (f = 0; f < frames; f++) {
		for (c = 0; c < channels; c++) {
			float *plane = (float *)audio->data[c];
			plane[f] = filter->scratch[f * channels + c];
		}
	}
	LeaveCriticalSection(&filter->lock);
	return audio;
}

struct obs_source_info airlock_delay_filter = {
	.id = "airlock_delay_filter",
	.type = OBS_SOURCE_TYPE_FILTER,
	.output_flags = OBS_SOURCE_AUDIO,
	.get_name = delay_name,
	.create = delay_create,
	.destroy = delay_destroy,
	.get_defaults = delay_defaults,
	.get_properties = delay_properties,
	.update = delay_update,
	.filter_audio = delay_audio,
};

OBS_DECLARE_MODULE()

bool obs_module_load(void)
{
	obs_register_source(&airlock_delay_filter);
	return true;
}
