# Included at the end of whisper.cpp's project() call when the desktop app
# is built with the cuda feature. The RTX 5090 is Blackwell (compute 12.0).
# The -fPIC flag whisper-rs passes is a GCC flag and breaks nvcc on Windows.
if(WIN32)
  set(CMAKE_CUDA_FLAGS "" CACHE STRING "" FORCE)
endif()
set(CMAKE_CUDA_ARCHITECTURES "120" CACHE STRING "Airlock speech GPU" FORCE)
