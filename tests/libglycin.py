#!/usr/bin/python3

import gi
import os
import os.path
import sys

gi.require_version("Gly", "1")
gi.require_version("GlyGtk4", "1")

from gi.repository import Gly, GlyGtk4, Gio, GLib

# test loader for color.jpg
def test_loader(loader):
    image = loader.load()
    test_image(image)

def test_image(image):
    mime_type = image.get_mime_type()

    assert mime_type == "image/jpeg", f"Wrong mime type {mime_type}"

    frame = image.next_frame()
    test_frame(frame)

def test_frame(frame):
    width = frame.get_width()
    height = frame.get_height()
    stride = frame.get_stride()
    first_byte = frame.get_buf_bytes().get_data()[0]
    memory_format = frame.get_memory_format()

    texture = GlyGtk4.frame_get_texture(frame)
    texture_width = texture.get_width()

    assert width == 600, f"Wrong width: {width} px"
    assert height == 400, f"Wrong height: {height} px"
    assert stride == 600 * 3, f"Wrong stride: {stride} px"
    assert first_byte > 50 and first_byte < 70, f"Wrong first byte: {first_byte}"
    assert memory_format == Gly.MemoryFormat.R8G8B8, f"Wrong memory format: {memory_format}"

    assert not Gly.MemoryFormat.has_alpha(memory_format)
    assert not Gly.MemoryFormat.is_premultiplied(memory_format)

    assert texture_width == 600, f"Wrong texture width: {texture_width} px"

def main():
    GLib.timeout_add_seconds(interval = 2, function = cb_exit)

    dir = os.path.dirname(os.path.abspath(__file__))

    test_image = os.path.join(dir, "test-images/images/color/color.jpg")
    file = Gio.File.new_for_path(test_image)

    # Types

    assert Gly.SandboxSelector.AUTO.__gtype__.name == "GlySandboxSelector"
    assert Gly.MemoryFormat.G8.__gtype__.name == "GlyMemoryFormat"
    assert Gly.MemoryFormatSelection.G8.__gtype__.name == "GlyMemoryFormatSelection"

    # Sync basics

    loader = Gly.Loader(file=file)
    loader.set_sandbox_selector(Gly.SandboxSelector.AUTO)

    test_loader(loader)

    # Loader constructors/sources

    loader = Gly.Loader.new(file)
    image = loader.load()
    frame = image.next_frame()
    assert frame.get_width() == 600

    loader = Gly.Loader.new_for_stream(file.read())
    test_loader(loader)

    loader = Gly.Loader(stream=file.read())
    test_loader(loader)

    with open(test_image, 'rb') as f:
        bytes = GLib.Bytes.new(f.read())

    loader = Gly.Loader.new_for_bytes(bytes)
    test_loader(loader)

    loader = Gly.Loader(bytes=bytes)
    test_loader(loader)

    # Memory selection

    loader = Gly.Loader(file=file)
    loader.set_accepted_memory_formats(Gly.MemoryFormatSelection.G8)

    image = loader.load()
    frame = image.next_frame()

    memory_format = frame.get_memory_format()

    assert memory_format == Gly.MemoryFormat.G8, f"Memory format was not accepted: {memory_format}"

    # Async

    loader = Gly.Loader(file=file)
    loader.set_sandbox_selector(Gly.SandboxSelector.AUTO)

    image = loader.load_async(None, loader_cb, "loader_data")

    GLib.MainLoop().run()

def loader_cb(loader, result, user_data):
    assert user_data == "loader_data"
    image = loader.load_finish(result)
    image.next_frame_async(None, image_cb, "image_data")

def image_cb(image, result, user_data):
    assert user_data == "image_data"
    frame = image.next_frame_finish(result)

    assert image.get_mime_type() == "image/jpeg"

    test_frame(frame)

    sys.exit(0)

def cb_exit():
    print("Test: Exiting after predefined waiting time.", file=sys.stderr)
    sys.exit(1)

if __name__ == "__main__":
    main()