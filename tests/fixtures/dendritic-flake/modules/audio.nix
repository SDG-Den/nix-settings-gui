{ self, inputs, ... }: {

  /* @menu
     displayname = "Audio";
     icon = "audio-card-symbolic";
     order = 40;
     category = "Hardware";
     systems = [ "alpha" ];
     description = "Sound server, only on alpha";
  */

  flake.modules.nixos.audio = { config, pkgs, ... }: {

    /* @option widget = "switch"; label = "Enable PipeWire"; */
    services.pipewire.enable = true;

    /* @option widget = "switch"; label = "Disable PulseAudio"; */
    services.pulseaudio.enable = false;
  };
}
