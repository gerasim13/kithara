package com.kithara.nativetest;

import android.app.Activity;
import android.app.Instrumentation;
import android.content.Context;
import android.os.Bundle;
import com.kithara.net.NativeHttpTransport;
import com.kithara.okhttp.OkHttpTransport;
import okhttp3.OkHttpClient;
import org.json.JSONArray;
import org.json.JSONObject;
import java.io.File;
import java.util.ArrayList;
import java.util.List;

public final class NativeInstrumentation extends Instrumentation {
    private String request;

    private static native int runNative(
        Context context, String[] args, String logPath, String directory, String[] environment);

    @Override
    public void onCreate(Bundle arguments) {
        super.onCreate(arguments);
        request = arguments.getString("request");
        start();
    }

    @Override
    public void onStart() {
        Bundle result = new Bundle();
        try {
            JSONObject input = new JSONObject(request);
            Context context = getTargetContext().getApplicationContext();
            if (context == null) throw new IllegalStateException("Application Context is missing");
            String library = input.getString("library");
            if (!library.matches("kithara_test_[0-9a-f]+"))
                throw new IllegalArgumentException("Invalid test library name");
            int code;
            if (input.getBoolean("executable")) {
                code = runExecutable(context, library, input);
            } else {
                System.loadLibrary(library);
                if (input.getBoolean("transport"))
                    NativeHttpTransport.install(new OkHttpTransport(new OkHttpClient()));
                code = runNative(context, strings(input.getJSONArray("args")),
                    input.getString("log"), input.getString("directory"),
                    strings(input.getJSONArray("environment")));
            }
            result.putInt("rust_exit_code", code);
            finish(code == 0 ? Activity.RESULT_OK : Activity.RESULT_CANCELED, result);
        } catch (Throwable error) {
            result.putString("bootstrap_error", error.toString());
            finish(Activity.RESULT_CANCELED, result);
        }
    }

    private static int runExecutable(Context context, String library, JSONObject input)
        throws Exception {
        String directory = context.getApplicationInfo().nativeLibraryDir;
        List<String> command = new ArrayList<>();
        command.add(new File(directory, "lib" + library + ".so").getPath());
        String[] arguments = strings(input.getJSONArray("args"));
        for (int i = 1; i < arguments.length; ++i) command.add(arguments[i]);
        ProcessBuilder process = new ProcessBuilder(command)
            .directory(new File(input.getString("directory")))
            .redirectErrorStream(true)
            .redirectOutput(new File(input.getString("log")));
        String[] environment = strings(input.getJSONArray("environment"));
        if (environment.length % 2 != 0)
            throw new IllegalArgumentException("Environment must contain name/value pairs");
        for (int i = 0; i < environment.length; i += 2)
            process.environment().put(environment[i], environment[i + 1]);
        process.environment().put("LD_LIBRARY_PATH", directory);
        return process.start().waitFor();
    }

    private static String[] strings(JSONArray values) throws org.json.JSONException {
        String[] result = new String[values.length()];
        for (int i = 0; i < result.length; ++i) result[i] = values.getString(i);
        return result;
    }
}
