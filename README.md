### Water Controller Firmware

Water controller utilizes a WESP32 ESP32 microcontroller with POE ethernet interface.

It monitors the DFRobot 80G millmeter radar sensor to measure the water level in the tank and monitors the water pressure to detect water cutoff events.

It also has a linear voltage pressure sensor that works off 5V supply and returns 0.5V for 0 psi and 4.5V for 100 psi max.

It displays the water level and pressure on a Sharp Memory LCD display.

Has MQTT integration with Home Assistant to publish sensor data and set parameters for run time.
