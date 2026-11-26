"""Constants for the Puzzlebox bridge."""

DOMAIN = "puzzlebox"

CONF_URL = "url"
CONF_TOKEN = "token"

SIGNAL_PROGRAMS = f"{DOMAIN}_programs"
SIGNAL_CONNECTION = f"{DOMAIN}_connection"
SIGNAL_VARIABLES = f"{DOMAIN}_variables"

EVENT_RUN_STARTED = "puzzlebox_program_started"
EVENT_RUN_FINISHED = "puzzlebox_program_finished"

PENDING_PACKS = f"{DOMAIN}_pending_packs"
