# Evaluates the Home Manager module with stand-ins for the few Home Manager
# options it sets, and checks the managed.json it writes.
{ pkgs, self }:

let
  inherit (pkgs) lib;
  files = lib.mkOption {
    type = lib.types.attrsOf (
      lib.types.submodule { options.source = lib.mkOption { type = lib.types.path; }; }
    );
    default = { };
  };
  eval = lib.evalModules {
    modules = [
      self.homeModules.default
      {
        options.home.packages = lib.mkOption {
          type = lib.types.listOf lib.types.package;
          default = [ ];
        };
        options.home.file = files;
        options.xdg.configFile = files;
        config._module.args.pkgs = pkgs;
        config.programs.ond = {
          enable = true;
          settings = {
            theme = "dark";
            playback.kbps = 192;
            device_modes.umc = "bit-perfect";
          };
        };
      }
    ];
  };
  inherit (eval) config;
  managed =
    if pkgs.stdenv.hostPlatform.isDarwin then
      config.home.file."Library/Application Support/dev.solidsilver.ond/managed.json".source
    else
      config.xdg.configFile."ond/managed.json".source;
in
assert pkgs.stdenv.hostPlatform.isLinux -> config.home.packages != [ ];
pkgs.runCommand "ond-hm-module-check" { nativeBuildInputs = [ pkgs.jq ]; } ''
  jq -e '.theme == "dark" and .playback.kbps == 192 and .device_modes.umc == "bit-perfect"' ${managed}
  touch $out
''
