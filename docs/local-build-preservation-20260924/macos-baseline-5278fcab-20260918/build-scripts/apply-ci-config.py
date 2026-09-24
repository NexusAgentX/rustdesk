from pathlib import Path
import re
root=Path(__file__).resolve().parent/'source'
changes={
'build.py':(r'MACOSX_DEPLOYMENT_TARGET=[0-9]+\.[0-9]+','MACOSX_DEPLOYMENT_TARGET=12.3'),
'flutter/macos/Podfile':(r"platform :osx, '.*'", "platform :osx, '12.3'"),
'Cargo.toml':(r'osx_minimum_system_version = "[0-9]+\.[0-9]+"','osx_minimum_system_version = "12.3"'),
'flutter/macos/Runner.xcodeproj/project.pbxproj':(r'MACOSX_DEPLOYMENT_TARGET = [0-9]+\.[0-9]+;', 'MACOSX_DEPLOYMENT_TARGET = 12.3;')}
for name,(pattern,replacement) in changes.items():
 p=root/name;p.write_text(re.sub(pattern,replacement,p.read_text()))
